import { createBuiltinChat } from '../lib/llm.mjs';
import { mergeUsage } from '../metrics/run.mjs';
import { taskAndRepairFiles, toolDefinitions, ToolAccessError, verificationDecision, isDocsOnlyScope } from './tools.mjs';
import { redactEvidence, taskSkipsTests } from './excellence.mjs';
import { parseTaskDocument } from '../planner/task.mjs';
import { readTaskMetadata } from './estimate.mjs';
import { applyReadmeStatus, hasRequiredReadmeStatus } from './readme-status.mjs';
import { ContractsSubmoduleError, onlyMissingContractsScripts } from '../lib/contracts.mjs';
import { UnsupportedFinishReasonError } from '../llm/finish-reason.mjs';
import { throwIfCancelled } from './cancel.mjs';
import { SteeringInterrupt } from './steering.mjs';

class MalformedCoderTools extends Error {}
export const testRepairBudget = 4;

function repairBudgetFor(task, bounded) {
  if (!bounded) return testRepairBudget;
  const difficulty = readTaskMetadata(task).difficulty;
  if (difficulty <= 2) return 1;
  if (difficulty === 3) return 2;
  return testRepairBudget;
}

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
  const singleAllowedFile = parsedTask.files_allowed.length === 1
    ? parsedTask.files_allowed[0] : null;
  const boundedTask = singleAllowedFile !== null;
  const repairBudget = repairBudgetFor(context.task, boundedTask);

  const verification = verificationDecision(parsedTask.files_allowed);
  const docsOnly = isDocsOnlyScope(parsedTask.files_allowed);
  const messages = [
    { role: 'system', content: context.pack },
    { role: 'user', content: 'Complete this task using only the offered tools. ' +
      (docsOnly ? 'This is a docs-only change. Do not run or edit tests. Check the written file. ' : '') +
      (!docsOnly && verification.run ? `Run and update only these tests: ${verification.update.join(', ')}. ` : '') +
      'A failing test outside Allowed Files is pre-existing: report it and do not edit it. ' +
      'Finish with a concise summary of changes, test results, and blockers.' },
  ];
  const requiresWebSearch = /\bweb_search\b/.test(context.task);
  const requiresWebFetch = /\bweb_fetch\b/.test(context.task);
  const definitions = toolDefinitions.filter((tool) =>
    (config.seat.tools.includes(tool.function.name) ||
      ['edit_file', 'glob_files', ...(requiresWebSearch || requiresWebFetch ? [] : ['run_command'])].includes(tool.function.name) ||
      (config.tools?.internet === true && ['web_search', 'web_fetch'].includes(tool.function.name))) &&
    (tool.function.name !== 'run_test' || verification.run && config.tools?.run_test !== false) &&
    (!boundedTask || ['read_file', 'write_file', 'edit_file', 'run_test', 'web_search', 'web_fetch'].includes(tool.function.name))).map((tool) =>
    !boundedTask || tool.function.name === 'run_test' || tool.function.name === 'web_search' || tool.function.name === 'web_fetch' ? tool : {
      ...tool, function: { ...tool.function, parameters: { ...tool.function.parameters,
        properties: { ...tool.function.parameters.properties,
          path: { type: 'string', enum: tool.function.name === 'read_file'
            ? ['TASK.md', singleAllowedFile] : [singleAllowedFile] },
        },
      } },
    });
  const offeredTools = new Set(definitions.map((tool) => tool.function.name));
  if (readmeOnlyDocs && !offeredTools.has('write_file')) {
    throw new Error('README-only docs task requires write_file; enable it before running the coder');
  }
  const chat = createBuiltinChat(config, { fetchImpl, env, vault, onEvent, retryCommand, signal, stream: true });
  const usages = [];
  const ids = new Set();
  let repaired = false;
  let needsTools = false;
  let attemptTurns = 0;
  let finalSummaryOnly = false;
  let checksPassedAfterWrite = false;
  let lateWriteReturned = false;
  let steeringMessage;
  let webSearchDone = false;
  let webFetchDone = false;
  progress.testRepairs = 0;
  progress.testRepairBudget = repairBudget;
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
    progress.repairFiles = (tests.repair_files ?? []).filter((file) => parsedTask.files_allowed.includes(file));
    const outside = (tests.repair_files ?? []).filter((file) => !parsedTask.files_allowed.includes(file));
    const output = redactEvidence([tests.stdout, tests.stderr].filter(Boolean).join('\n'), {
      env, apiKeyEnv: config.llm.api_key_env,
    }).slice(0, 4096);
    const failure = `Final node --test failed (exit ${tests.exit_code}):\n${output}`;
    if (outside.length && !progress.repairFiles.length) {
      progress.baselineFailures = outside;
      if (progress.outsideRefocus) {
        await onEvent?.({ type: 'remediation-offer', files: outside });
        messages.push({ role: 'user', content: `Baseline failures remain outside Allowed Files (${outside.join(', ')}). ` +
          'Do not edit them. Summarize the task and say a separate remediation agent can take those files if the human asks.' });
        return false;
      }
      progress.outsideRefocus = true;
      attemptTurns = 0;
      finalSummaryOnly = false;
      await onEvent?.({ type: 'steering' });
      messages.push({ role: 'user', content: `Steering: these failing tests are outside Allowed Files and are pre-existing: ${outside.join(', ')}. ` +
        'Do not read or edit them. Continue only on the allowed files. ' +
        'A remediation agent is a separate session and starts only if the human asks.' });
      return true;
    }
    if (progress.testRepairs === repairBudget) {
      progress.repairBudgetExhausted = true;
      throw new Error(`${failure}\nTest repair budget (${repairBudget}) exhausted`);
    }
    progress.testRepairs += 1;
    attemptTurns = 0;
    finalSummaryOnly = false;
    checksPassedAfterWrite = false;
    await onEvent?.({ type: 'test-repair', attempt: progress.testRepairs, budget: repairBudget });
    messages.push({ role: 'user', content: `${failure}\n` +
      `Repair ${progress.testRepairs} of ${repairBudget}. Repair only TASK-allowed files` +
      (progress.repairFiles.length ? `: ${progress.repairFiles.join(', ')}` : '') +
      '. Do not edit a failing test outside Allowed Files. Report it as pre-existing. ' +
      'Rerun node --test, then provide a new summary. No change is verified yet.' });
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
      if (error instanceof UnsupportedFinishReasonError && error.truncated && !progress.lengthContinued) {
        progress.lengthContinued = true;
        messages.push({ role: 'user', content: 'The response stopped at the completion cap. Continue with one complete tool call or a summary. Do not repeat completed edits.' });
        continue;
      }
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
        const returnToDraft = !lateWriteReturned && message.tool_calls.length === 1 &&
          message.tool_calls[0]?.function?.name === 'write_file';
        if (returnToDraft) {
          lateWriteReturned = true;
          finalSummaryOnly = false;
          checksPassedAfterWrite = false;
          attemptTurns = 0;
          calls = decodeCalls(message, offeredTools, ids, turn);
        } else {
          for (const call of message.tool_calls) {
            await onEvent?.({ type: 'tool-result', name: call?.function?.name ?? 'unknown', status: 'denied' });
          }
          deterministic = `Updated ${singleAllowedFile}. Task checks passed.`;
          calls = [];
        }
      } else {
        if (boundedTask && Array.isArray(message.tool_calls) && message.tool_calls
          .some((call) => ['list_dir', 'search_text'].includes(call?.function?.name))) {
          calls = decodeCalls(message, offeredTools, ids, turn);
          for (const call of calls) {
            messages.push({ role: 'tool', tool_call_id: call.id,
              content: 'Denied by the task scope. Continue with an allowed read or write, or summarize the blocker.' });
            await onEvent?.({ type: 'tool-result', name: call.function.name, status: 'denied' });
          }
          continue;
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
      if (!offeredTools.has('write_file') || !offeredTools.has('read_file') || !readmeOnlyDocs) {
        messages.push({ role: 'user', content: 'The last response was not a valid tool call. Continue with one allowed tool call, or summarize the blocker. Do not repeat the same text.' });
        continue;
      }
      deterministic = await applyReadmeStatus({ task: context.task, tools });
      progress.implementationPath = 'deterministic-readme';
      await onEvent?.({ type: 'implementation', path: 'deterministic-readme' });
      calls = [];
    }
    if (calls.length) {
      if (finalSummaryOnly) throw new Error('Coder requested tools instead of the required final summary after green repair tests');
      if (progress.testRepairs === 0 && attemptTurns === config.seat.turn_budget + Number(repaired)) {
        const exploring = calls.every((call) => ['read_file', 'list_dir', 'search_text'].includes(call.function.name));
        if (exploring && !progress.writeForced) {
          progress.writeForced = true;
          attemptTurns = Math.max(0, config.seat.turn_budget - 3);
          for (const call of calls) {
            messages.push({ role: 'tool', tool_call_id: call.id,
              content: 'Exploration budget is spent. Next response must be write_file for the allowed files only.' });
          }
          messages.push({ role: 'user', content: 'Stop reading and searching. Write the allowed files now, then summarize.' });
          continue;
        }
        throw new Error(`Coder turn budget (${config.seat.turn_budget}) exhausted before a summary`);
      }
      if (finishReason === 'stop') throw new Error('LLM coder stopped while requesting tools');
      calls.forEach((call) => ids.add(call.id));
      messages.push({ role: 'assistant', content: message.content ?? null,
        tool_calls: calls.map(({ args: _args, ...call }) => call) });
      let failedTests;
      let wroteSingleAllowedFile = false;
      for (const [index, call] of calls.entries()) {
        const savesNamedFileLater = singleAllowedFile && call.function.name === 'run_test' &&
          calls.slice(index + 1).some((pending) =>
            pending.function.name === 'write_file' && pending.args.path === singleAllowedFile);
        if (!wroteSingleAllowedFile && savesNamedFileLater) {
          messages.push({ role: 'tool', tool_call_id: call.id,
            content: `Deferred until ${singleAllowedFile} is saved.` });
          await onEvent?.({ type: 'tool-result', name: call.function.name, status: 'denied' });
          continue;
        }
        if (wroteSingleAllowedFile && call.function.name !== 'write_file') {
          messages.push({ role: 'tool', tool_call_id: call.id,
            content: 'Refused after the successful sole-file write; task checks run next.' });
          await onEvent?.({ type: 'tool-result', name: call.function.name, status: 'denied' });
          continue;
        }
        if ((call.function.name === 'edit_file' || call.function.name === 'write_file') &&
            /(?:^|\/)(?:test(?:[._-][^/]+)?|[^/]+[._-]test)\.[cm]?js$/.test(String(call.args.path ?? '')) &&
            !verification.update.includes(String(call.args.path).replaceAll('\\', '/'))) {
          messages.push({ role: 'tool', tool_call_id: call.id,
            content: 'Denied. Update only a test that covers a file changed in this session.' });
          await onEvent?.({ type: 'tool-result', name: call.function.name, status: 'denied' });
          continue;
        }
        if (call.function.name === 'run_test' && progress.baselineFailures?.length) {
          messages.push({ role: 'tool', tool_call_id: call.id,
            content: 'Baseline failures were already reported. Do not run tests again. Return the summary only.' });
          finalSummaryOnly = true;
          continue;
        }
        if (call.function.name === 'run_test' && !verification.run) {
          messages.push({ role: 'tool', tool_call_id: call.id,
            content: 'Denied. This task requires web_search and web_fetch before tests. Record the query, title, and URL, then write the allowed file.' });
          await onEvent?.({ type: 'tool-result', name: call.function.name, status: 'denied' });
          continue;
        }
        let result;
        try {
          result = await tools[call.function.name](call.args);
        } catch (error) {
          messages.push({ role: 'tool', tool_call_id: call.id,
            content: `Denied: ${error instanceof Error ? error.message : 'tool failed'}. Continue with an allowed action or summarize the blocker.` });
          await onEvent?.({ type: 'tool-result', name: call.function.name, status: 'denied' });
          continue;
        }
        messages.push({
          role: 'tool', tool_call_id: call.id,
          content: redactEvidence(typeof result === 'string' ? result : JSON.stringify(result), {
            env, apiKeyEnv: config.llm.api_key_env,
          }),
        });
        if (call.function.name === 'web_search') webSearchDone = true;
        if (call.function.name === 'web_fetch') webFetchDone = true;
        if (call.function.name === 'write_file' && result.path === singleAllowedFile) {
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
        const testsSkipped = !verification.run || taskSkipsTests(context.task);
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
      if (checksPassedAfterWrite || progress.tests?.exit_code === 0) {
        deterministic = 'Updated the allowed file. Task checks passed.';
      } else if (!progress.summaryPrompted) {
        progress.summaryPrompted = true;
        messages.push({ role: 'user', content: 'Return a final summary of the files changed and the test result. Do not call tools.' });
        continue;
      } else {
        deterministic = 'Coder finished without a prose summary. Review the written files and the test result.';
      }
    }
    const usage = mergeUsage(...usages);
    const summary = deterministic ?? message.content.trim();
    if (progress.baselineFailures?.length && !progress.repairFiles.length) {
      return {
        mode: 'llm', model: config.llm.model, summary, usage, turns: progress.turns,
        tests: progress.tests, testsSkipped: false, baselineFailures: progress.baselineFailures,
        implementationPath: progress.implementationPath ?? 'model',
        testRepairs: progress.testRepairs, repairFiles: progress.repairFiles,
      };
    }
    const testsSkipped = !verification.run || taskSkipsTests(context.task);
    const tests = checksPassedAfterWrite ? progress.tests
      : testsSkipped && progress.testRepairs === 0 ? undefined : await tools.run_test({});
    progress.tests = tests;
    const result = {
      mode: 'llm', model: config.llm.model, summary, usage, turns: progress.turns, tests, testsSkipped,
      implementationPath: progress.implementationPath ?? 'model',
      testRepairs: progress.testRepairs, repairFiles: progress.repairFiles,
    };
    if (tests && tests.exit_code !== 0) {
      messages.push({ role: 'assistant', content: summary });
      const again = await repairTests(tests);
      if (!again && progress.baselineFailures?.length) return result;
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
    const missingWrite = reasons.find((reason) => reason.includes('must write '));
    if (missingWrite && progress.testRepairs < repairBudget) {
      progress.testRepairs += 1;
      attemptTurns = 0;
      finalSummaryOnly = false;
      checksPassedAfterWrite = false;
      messages.push({ role: 'assistant', content: summary });
      messages.push({ role: 'user', content: `${missingWrite}\nWrite the allowed file, rerun the required test, then summarize. The previous summary is not a result.` });
      continue;
    }
    const unsafe = reasons.find((reason) => !reason.startsWith('node --test failed (exit ') && !reason.includes('must write '));
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
