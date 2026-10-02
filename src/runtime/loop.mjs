import { createBuiltinChat } from '../lib/llm.mjs';
import { mergeUsage } from '../metrics/run.mjs';
import { taskAndRepairFiles, toolDefinitions, ToolAccessError } from './tools.mjs';
import { redactEvidence, taskSkipsTests } from './excellence.mjs';
import { parseTaskDocument } from '../planner/task.mjs';
import { applyReadmeStatus, hasRequiredReadmeStatus } from './readme-status.mjs';
import { ContractsSubmoduleError, onlyMissingContractsScripts } from '../lib/contracts.mjs';
import { UnsupportedFinishReasonError } from '../llm/finish-reason.mjs';
import { throwIfCancelled } from './cancel.mjs';
import { SteeringInterrupt } from './steering.mjs';
import { readTaskMetadata } from './estimate.mjs';

class MalformedCoderTools extends Error {}
export const testRepairBudget = 4;

function decodeCalls(message, offered, ids, turn) {
  if (message.tool_calls !== undefined && !Array.isArray(message.tool_calls)) {
    throw new MalformedCoderTools('LLM coder returned malformed tool calls');
  }
  const batchIds = new Set(ids);
  return (message.tool_calls ?? []).map((call, index) => {
    if (!call || call.type !== 'function' || !offered.has(call.function?.name) ||
        call.id !== undefined && (typeof call.id !== 'string' || !call.id || batchIds.has(call.id))) {
      throw new Error('LLM coder requested an invalid or unavailable tool');
    }
    if (typeof call.function.arguments !== 'string') throw new MalformedCoderTools('Coder tool arguments must be a JSON string');
    let args;
    try { args = JSON.parse(call.function.arguments); }
    catch { throw new MalformedCoderTools('Coder tool arguments are not valid JSON'); }
    if (!args || typeof args !== 'object' || Array.isArray(args)) throw new MalformedCoderTools('Coder tool arguments must be an object');
    const id = call.id ?? `coder-${turn}-${index}`;
    batchIds.add(id);
    return { id, type: 'function', function: call.function, args };
  });
}

function stubSummary(task) {
  const { title, acceptance_checks: checks } = parseTaskDocument(task);
  return `Task: ${title}\n\nAcceptance checks:\n${checks.map((check) => `- ${check}`).join('\n')}\n\n` +
    'Deterministic stub only: no implementation or tests were run. Configure llm.base_url to run a coder.';
}

async function executeLoop({ config, context, tools, fetchImpl, env, vault, verify, onEvent, retryCommand, signal, steeringControl }, progress) {
  throwIfCancelled(signal);
  if (!config.llm.base_url) {
    const summary = stubSummary(context.task);
    return {
      mode: 'stub', summary, usage: null, turns: 0,
    };
  }
  if (typeof verify !== 'function') {
    throw new TypeError('Configured coder requires an excellence verifier');
  }
  if (config.tools?.run_test === false && !taskSkipsTests(context.task)) {
    throw new Error('run_test is disabled by tools.run_test; enable it or explicitly declare TASK.md tests: none');
  }
  const readmeOnlyDocs = context.contextPolicy?.readmeOnlyDocs === true;
  const sliceReadsOnly = context.contextPolicy?.sliceReadsOnly === true;
  const parsedTask = parseTaskDocument(context.task);
  const singleAllowedFile = readTaskMetadata(context.task).task_class === 'docs' &&
    parsedTask.files_allowed.length === 1
    ? parsedTask.files_allowed[0] : null;

  const messages = [
    { role: 'system', content: context.pack },
    { role: 'user', content: 'Complete this task using only the offered tools. ' +
      (context.minimalDocs ? 'Use only the Ask, TASK.md, allowed files, read-before-write, and small-diff. ' +
        'Read task-allowed files before edits; no principal or research pack is needed. '
        : 'Read RESEARCH.md for the pre-edit inventory and gaps. ') +
      (readmeOnlyDocs ? 'Read only TASK.md or README.md; write README.md before tests or finishing. ' : '') +
      (sliceReadsOnly ? 'Reads, listings, and searches are limited to TASK.md and TASK-allowed paths. ' : '') +
      'Do not claim acceptance checks passed without evidence. ' +
      'Finish with a concise summary of changes, test results, and blockers.' },
  ];
  const definitions = toolDefinitions.filter((tool) => config.seat.tools.includes(tool.function.name) &&
    (tool.function.name !== 'run_test' || config.tools?.run_test !== false) &&
    (!readmeOnlyDocs || ['read_file', 'write_file', 'run_test'].includes(tool.function.name))).map((tool) =>
    !readmeOnlyDocs || tool.function.name === 'run_test' ? tool : {
      ...tool, function: { ...tool.function, parameters: { ...tool.function.parameters,
        properties: { ...tool.function.parameters.properties,
          path: { type: 'string', enum: tool.function.name === 'read_file' ? ['TASK.md', 'README.md'] : ['README.md'] },
        },
      } },
    });
  const offeredTools = new Set(definitions.map((tool) => tool.function.name));
  if (readmeOnlyDocs && !offeredTools.has('write_file')) {
    throw new Error('README-only docs task requires write_file; enable it before running the coder');
  }
  const chat = createBuiltinChat(config, { fetchImpl, env, vault, onEvent, retryCommand, signal });
  const usages = [];
  const ids = new Set();
  let repaired = false;
  let needsTools = false;
  let attemptTurns = 0;
  let finalSummaryOnly = false;
  let checksPassedAfterWrite = false;
  let steeringMessage;
  progress.testRepairs = 0;
  progress.repairFiles = [];
  const repairTests = async (tests) => {
    if (!Number.isSafeInteger(tests?.exit_code) || tests.exit_code < 0) {
      throw new TypeError('run_test must return a nonnegative integer exit_code');
    }
    progress.tests = tests;
    if (tests.exit_code === 0) return false;
    if (onlyMissingContractsScripts(tests)) {
      await onEvent?.({ type: 'contracts-uninitialized' });
      throw new ContractsSubmoduleError({ tests: { exit_code: tests.exit_code } });
    }
    progress.repairFiles = taskAndRepairFiles([], tests.repair_files ?? progress.repairFiles);
    const output = redactEvidence([tests.stdout, tests.stderr].filter(Boolean).join('\n'), {
      env, apiKeyEnv: config.llm.api_key_env,
    }).slice(0, 4096);
    const failure = `Final node --test failed (exit ${tests.exit_code}):\n${output}`;
    if (progress.testRepairs === testRepairBudget) {
      progress.repairBudgetExhausted = true;
      throw new Error(`${failure}\nTest repair budget (${testRepairBudget}) exhausted`);
    }
    progress.testRepairs += 1;
    attemptTurns = 0;
    finalSummaryOnly = false;
    checksPassedAfterWrite = false;
    await onEvent?.({ type: 'test-repair', attempt: progress.testRepairs, budget: testRepairBudget });
    messages.push({ role: 'user', content: `${failure}\n` +
      `Repair ${progress.testRepairs} of ${testRepairBudget}. Read this failure summary and repair TASK-allowed files` +
      (progress.repairFiles.length ? ` plus the failing tests: ${progress.repairFiles.join(', ')}` : '') +
      '. Rerun node --test, then provide a new summary. No change is verified yet.' });
    return true;
  };
  await onEvent?.({ type: 'implementation', path: 'model' });
  for (;;) {
    throwIfCancelled(signal);
    const instruction = steeringControl?.take();
    if (instruction) {
      const previous = steeringMessage ? messages.indexOf(steeringMessage) : -1;
      if (previous >= 0) messages.splice(previous, 1);
      steeringMessage = { role: 'user', content: `Human steering (task scope is unchanged):\n${instruction}\n` +
        'Do not change TASK.md or Allowed Files. Use only the existing offered tools and allowed paths. ' +
        'Any earlier summary-only instruction is withdrawn; read allowed files before edits and rerun required tests.' };
      messages.push(steeringMessage);
      finalSummaryOnly = false;
      needsTools = false;
      await onEvent?.({ type: 'steering' });
    }
    if (attemptTurns === config.seat.turn_budget + Number(repaired)) {
      if (progress.testRepairs === 0 || finalSummaryOnly) {
        throw new Error(`Coder turn budget (${config.seat.turn_budget}) exhausted`);
      }
      if (await repairTests(await tools.run_test({}))) continue;
      attemptTurns = 0;
      finalSummaryOnly = true;
      needsTools = false;
      messages.push({ role: 'user', content: 'Repair tests are green. Return a final summary only; do not request more tools.' });
    }
    attemptTurns += 1;
    progress.turns += 1;
    const turn = progress.turns;
    progress.usage = null;
    const scopedDefinitions = readmeOnlyDocs && progress.repairFiles.length ? definitions.map((tool) =>
      ['read_file', 'write_file'].includes(tool.function.name) ? {
        ...tool, function: { ...tool.function, parameters: { ...tool.function.parameters,
          properties: { ...tool.function.parameters.properties,
            path: { type: 'string', enum: [
              ...(tool.function.name === 'read_file' ? ['TASK.md', 'README.md'] : ['README.md']),
              ...progress.repairFiles,
            ] },
          },
        } },
      } : tool) : definitions;
    const currentDefinitions = finalSummaryOnly && checksPassedAfterWrite ? [] : scopedDefinitions;
    let response;
    let acceptedLateLength = false;
    try {
      response = steeringControl ? await steeringControl.request((requestSignal) =>
        chat({ messages, tools: currentDefinitions }, { signal: requestSignal }))
        : await chat({ messages, tools: currentDefinitions });
    } catch (error) {
      progress.turns += Math.max(0, chat.lastAttempts - 1);
      progress.response = chat.lastResponse ?? progress.response;
      progress.usage = mergeUsage(...usages, chat.lastUsage);
      if (error instanceof SteeringInterrupt) {
        usages.push(chat.lastUsage);
        attemptTurns = Math.max(0, attemptTurns - 1);
        continue;
      }
      if (error instanceof UnsupportedFinishReasonError && error.truncated && chat.lastAttempts === 2 &&
          readmeOnlyDocs && await hasRequiredReadmeStatus({ task: context.task, tools })) {
        acceptedLateLength = true;
        usages.push(chat.lastUsage);
        response = { finish_reason: 'stop', message: { role: 'assistant',
          content: 'README.md already contains the required one-line Status section' +
            (checksPassedAfterWrite ? '; task checks passed.' : '.') },
        usage: chat.lastUsage };
      } else {
        throw error;
      }
    }
    if (!acceptedLateLength) {
      progress.turns += chat.lastAttempts - 1;
      progress.response = chat.lastResponse;
      usages.push(response.usage);
      progress.usage = mergeUsage(...usages);
    }
    const { message, finish_reason: finishReason } = response;
    if (!message || typeof message !== 'object' || !['stop', 'tool_calls', undefined, null]
      .includes(finishReason)) {
      throw new Error('LLM coder returned an unsupported chat response');
    }
    if (finalSummaryOnly && !checksPassedAfterWrite && (finishReason === 'tool_calls' ||
        message.tool_calls !== undefined && (!Array.isArray(message.tool_calls) || message.tool_calls.length) ||
        typeof message.content !== 'string' || !message.content.trim())) {
      throw new Error('Coder must return a final summary without tools after green repair tests');
    }
    let calls;
    let deterministic = acceptedLateLength ? message.content : undefined;
    try {
      if (acceptedLateLength) {
        calls = [];
      } else if (finalSummaryOnly && checksPassedAfterWrite && Array.isArray(message.tool_calls) &&
          message.tool_calls.length) {
        for (const call of message.tool_calls) {
          await onEvent?.({ type: 'tool-result', name: call?.function?.name ?? 'unknown', status: 'denied' });
        }
        deterministic = `Updated ${singleAllowedFile}. Task checks passed.`;
        calls = [];
      } else {
        if (readmeOnlyDocs && Array.isArray(message.tool_calls) && message.tool_calls
          .some((call) => ['list_dir', 'search_text'].includes(call?.function?.name))) {
          throw new ToolAccessError('README-only docs task does not allow directory listing or repository search');
        }
        calls = decodeCalls(message, offeredTools, ids, turn);
        if ((needsTools || finishReason === 'tool_calls') && !calls.length) {
          throw new MalformedCoderTools('Coder repair did not emit tool calls');
        }
      }
      needsTools = false;
    } catch (error) {
      if (!(error instanceof MalformedCoderTools)) throw error;
      if (!repaired) {
        repaired = true;
        needsTools = true;
        messages.push({ role: 'user', content: 'Emit only tool_calls with JSON-string arguments for the offered tools.' });
        continue;
      }
      if (!offeredTools.has('write_file') || !offeredTools.has('read_file')) {
        throw new Error('Deterministic fallback needs the configured read_file and write_file permissions');
      }
      deterministic = await applyReadmeStatus({ task: context.task, tools });
      progress.implementationPath = 'deterministic-readme';
      await onEvent?.({ type: 'implementation', path: 'deterministic-readme' });
      calls = [];
    }
    if (calls.length) {
      if (finalSummaryOnly) throw new Error('Coder requested tools instead of the required final summary after green repair tests');
      if (progress.testRepairs === 0 && attemptTurns === config.seat.turn_budget + Number(repaired)) {
        throw new Error(`Coder turn budget (${config.seat.turn_budget}) exhausted before a summary`);
      }
      if (finishReason === 'stop') throw new Error('LLM coder stopped while requesting tools');
      calls.forEach((call) => ids.add(call.id));
      messages.push({ role: 'assistant', content: message.content ?? null,
        tool_calls: calls.map(({ args: _args, ...call }) => call) });
      let failedTests;
      let wroteSingleAllowedFile = false;
      for (const [index, call] of calls.entries()) {
        if (wroteSingleAllowedFile && call.function.name !== 'write_file') {
          messages.push({ role: 'tool', tool_call_id: call.id,
            content: 'Refused after the successful sole-file write; task checks run next.' });
          await onEvent?.({ type: 'tool-result', name: call.function.name, status: 'denied' });
          continue;
        }
        const result = await tools[call.function.name](call.args);
        messages.push({
          role: 'tool', tool_call_id: call.id,
          content: redactEvidence(typeof result === 'string' ? result : JSON.stringify(result), {
            env, apiKeyEnv: config.llm.api_key_env,
          }),
        });
        if (progress.testRepairs === 0 && call.function.name === 'write_file' &&
            result.path === singleAllowedFile) {
          wroteSingleAllowedFile = true;
          continue;
        }
        if (call.function.name === 'run_test') {
          progress.tests = result;
          if (result.exit_code !== 0) {
            failedTests = result;
            for (const pending of calls.slice(index + 1)) {
              messages.push({ role: 'tool', tool_call_id: pending.id,
                content: 'Deferred after failed tests; read the failure summary before more edits.' });
            }
            break;
          }
        }
      }
      if (failedTests) await repairTests(failedTests);
      if (wroteSingleAllowedFile) {
        const testsSkipped = taskSkipsTests(context.task);
        const tests = testsSkipped ? undefined : await tools.run_test({});
        progress.tests = tests;
        if (tests && tests.exit_code !== 0) {
          await repairTests(tests);
          continue;
        }
        checksPassedAfterWrite = true;
        finalSummaryOnly = true;
        attemptTurns = 0;
        needsTools = false;
        messages.push({ role: 'user', content: 'Task checks passed after the sole allowed file was saved. ' +
          'Return a final summary only; do not request more tools.' });
      }
      continue;
    }
    if (!deterministic && (finishReason === 'tool_calls' || typeof message.content !== 'string' ||
        !message.content.trim())) {
      throw new Error('LLM coder did not return a final summary');
    }
    const usage = mergeUsage(...usages);
    const testsSkipped = taskSkipsTests(context.task);
    const tests = checksPassedAfterWrite ? progress.tests
      : testsSkipped && progress.testRepairs === 0 ? undefined : await tools.run_test({});
    progress.tests = tests;
    const summary = deterministic ?? message.content.trim();
    const result = {
      mode: 'llm', model: config.llm.model, summary, usage, turns: progress.turns, tests, testsSkipped,
      implementationPath: progress.implementationPath ?? 'model',
      testRepairs: progress.testRepairs, repairFiles: progress.repairFiles,
    };
    if (tests && tests.exit_code !== 0) {
      messages.push({ role: 'assistant', content: summary });
      await repairTests(tests);
      continue;
    }
    const excellence = await verify(result);
    if (typeof excellence?.pass !== 'boolean' || !Array.isArray(excellence.reasons)) {
      throw new TypeError('Coder excellence verifier returned an invalid report');
    }
    if (excellence.pass && (!tests || tests.exit_code === 0)) return result;
    if (deterministic) throw new Error(`Deterministic fallback failed excellence: ${excellence.reasons[0]}`);

    const reasons = excellence.reasons.map((reason) => redactEvidence(reason, {
      env, apiKeyEnv: config.llm.api_key_env,
    }));
    const unsafe = reasons.find((reason) => !reason.startsWith('node --test failed (exit '));
    if (unsafe) throw new Error(`Coder excellence gate failed: ${unsafe}`);
    if (!tests || tests.exit_code === 0) {
      throw new Error('Coder excellence verifier did not confirm passing final tests');
    }
  }
}

export async function runLoop(options) {
  const progress = {
    mode: options.config.llm.base_url ? 'llm' : 'stub',
    model: options.config.llm.base_url ? options.config.llm.model : 'builtin-stub',
    turns: 0, usage: null, response: null,
  };
  try {
    const result = await executeLoop(options, progress);
    return { ...progress, ...result };
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    if (error instanceof ContractsSubmoduleError) {
      return { ...progress, tests: error.tests, blocked: true, summary: error.message, error };
    }
    if (error instanceof UnsupportedFinishReasonError) {
      return { ...progress, finishReason: error.finishReason, summary: error.message, error };
    }
    return { ...progress, summary: 'Coder execution stopped before a verified result.', error };
  }
}
