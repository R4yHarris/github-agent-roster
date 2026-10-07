import { createBuiltinChat } from '../lib/llm.mjs';
import { mergeUsage } from '../metrics/run.mjs';
import { recipeAllowsTool, taskAndRepairFiles, toolDefinitions, ToolAccessError, ToolUsageError, verificationDecision, isDocsOnlyScope, coversTestFile, testFailureEvidence } from './tools.mjs';
import { redactEvidence, taskSkipsTests } from './excellence.mjs';
import { parseTaskDocument } from '../planner/task.mjs';
import { readTaskMetadata } from './estimate.mjs';
import { applyReadmeStatus, hasRequiredReadmeStatus } from './readme-status.mjs';
import { ContractsSubmoduleError, onlyMissingContractsScripts } from '../lib/contracts.mjs';
import { UnsupportedFinishReasonError } from '../llm/finish-reason.mjs';
import { ChatError } from '../llm/request.mjs';
import { throwIfCancelled } from './cancel.mjs';
import { SteeringInterrupt } from './steering.mjs';

class MalformedCoderTools extends Error {}
const rosterToolNames = new Set(toolDefinitions.map((tool) => tool.function.name));
const safeToolName = (name) => String(name).replace(/[^\w.-]/g, '?').slice(0, 64);
// A hallucinated tool (not a Roster tool at all) gets one correction; a withheld Roster tool still fails the seat.
class UnknownCoderTool extends Error {
  constructor(name) {
    super(`LLM coder requested an invalid or unavailable tool: ${safeToolName(name)}`);
    this.toolName = safeToolName(name);
  }
}
export const testRepairBudget = 4;
// Progress extends the repair budget one repair at a time, never past this cap.
export const testRepairCap = 12;
// A context this full is handed to a fresh coder instead of being extended further.
export const repairContextShare = 0.5;
const maxRepeatedDenials = 2;
const sentinelGuidance = 'For secret-leak or redaction tests, feed an obvious non-credential sentinel such as ' +
  "'test-only-private-api-key' into the app code under test through a realistic secret context (an env secret or " +
  "an api_key assignment) and assert it is absent from that code's output; never special-case the sentinel in app code, and " +
  'never write sk-, ghp_, gho_, github_pat_ prefixed values or PEM private-key blocks.';

function mentionsSecrets(task) {
  return /\b(?:secrets?|credentials?|tokens?|api[ _-]?keys?|leaks?|leaking|redact\w*)\b/i.test(task);
}

function repairBudgetFor(task, bounded) {
  if (!bounded) return testRepairBudget;
  const difficulty = readTaskMetadata(task).difficulty;
  if (difficulty <= 2) return 1;
  if (difficulty === 3) return 2;
  return testRepairBudget;
}

// Failure identity without timings, so a changed failure signals repair progress.
function failureFingerprint(output) {
  return String(output).replace(/\(\d+(?:\.\d+)?m?s\)/g, '').replace(/duration_ms\s+\S+/g, '')
    .replace(/\s+/g, ' ').trim();
}

// The failing-test count from the node --test spec or TAP summary, when present.
export function failingTestCount(stdout) {
  const matches = [...String(stdout ?? '').matchAll(/^\s*(?:ℹ|#)\s*fail\s+(\d+)\s*$/gm)];
  return matches.length ? Number(matches.at(-1)[1]) : undefined;
}

function decodeCalls(message, offered, ids, turn) {
  if (message.tool_calls !== undefined && !Array.isArray(message.tool_calls)) {
    throw new MalformedCoderTools('LLM coder returned malformed tool calls');
  }
  const batchIds = new Set(ids);
  return (message.tool_calls ?? []).map((call, index) => {
    const name = call?.function?.name;
    if (call && call.type === 'function' && typeof name === 'string' && !offered.has(name) &&
        !rosterToolNames.has(name) && offered.size) {
      throw new UnknownCoderTool(name);
    }
    if (!call || call.type !== 'function' || !offered.has(name) ||
        call.id !== undefined && (typeof call.id !== 'string' || !call.id)) {
      throw new Error(`LLM coder requested an invalid or unavailable tool${typeof name === 'string' ? `: ${safeToolName(name)}` : ''}`);
    }
    // Gateways may restart call ids per response; a reused id is a transport quirk, so mint a fresh one.
    if (call.id !== undefined && batchIds.has(call.id)) call = { ...call, id: undefined };
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
  let repairBudget = repairBudgetFor(context.task, boundedTask);
  const explorationBudget = Math.min(config.seat.turn_budget,
    Math.max(4, parsedTask.files_allowed.length * 2));

  const verification = verificationDecision(parsedTask.files_allowed);
  const docsOnly = isDocsOnlyScope(parsedTask.files_allowed);
  const rules = [
    'Work to completion within the task boundary. When an attempt fails, use the new evidence to change strategy; ' +
      'do not repeat the same action against the same unchanged state. Consider a different implementation perspective ' +
      'before concluding that the bounded task cannot be completed.',
    docsOnly ? 'This is a docs-only change. Do not run or edit tests. Check the written file.' : '',
    !docsOnly && verification.run ? `Run and update only these tests: ${verification.update.join(', ')}` +
      ' (a test split into tests/<module>.<topic>.test.mjs shards counts as the same test).' : '',
    boundedTask && !docsOnly ? 'Read the allowed file and only direct imports needed to understand the APIs you will use; do not recursively trace transitive dependencies. ' +
      'Batch independent reads in one response, preserve existing imports and unrelated assertions, and make the smallest targeted edit.' : '',
    boundedTask && !docsOnly ? 'All tool paths are relative to the worktree root: an import like ../src/api.mjs from tests/test.mjs ' +
      'must be read as src/api.mjs, never ../src/api.mjs. Preserve existing behavior and implement the requested outcome; ' +
      'passing existing tests or rewriting identical content is not completion.' : '',
    mentionsSecrets(context.task) ? sentinelGuidance : '',
    'A failing test outside Allowed Files is pre-existing: report it and do not edit it.',
    !boundedTask && !docsOnly && (config.seat.scope_expansion ?? 3) > 0
      ? `Allowed Files are the planned scope. If the outcome truly requires another product file, you may write at most ` +
        `${config.seat.scope_expansion ?? 3} files outside it; each is recorded, judged by the reviewer, and listed in the PR, ` +
        'so justify each one in your summary. Secrets, .git, policy, workflows, vendor, and harness files stay denied.'
      : '',
  ].filter(Boolean);
  const messages = [
    { role: 'system', content: context.pack },
    { role: 'user', content: 'Complete this task using only the offered tools. ' +
      'Deliver each numbered TASK.md check; the task is done only when every item holds.\n\n' +
      parsedTask.acceptance_checks.map((check, index) => `${index + 1}. ${check}`).join('\n') +
      '\n\nRules:\n' + rules.map((rule) => `- ${rule}`).join('\n') +
      '\n\nFinish with a concise summary of changes, test results, and blockers, naming each numbered check as done or blocked.' },
  ];
  const requiresWebSearch = /\bweb_search\b/.test(context.task);
  const requiresWebFetch = /\bweb_fetch\b/.test(context.task);
  // run_command only runs git status/diff and node --test; offer it when TASK.md asks for one of those,
  // never alongside web research, whose untrusted pages must not steer commands.
  const requiresCommand = /\brun_command\b|\bgit (?:status|diff)\b/.test(context.task) &&
    !requiresWebSearch && !requiresWebFetch;
  const definitions = toolDefinitions.filter((tool) =>
    recipeAllowsTool(config.seat.recipe_tools, tool.function.name) &&
    (config.seat.tools.includes(tool.function.name) ||
      ['edit_file', 'delete_file', 'glob_files', ...(requiresCommand ? ['run_command'] : [])].includes(tool.function.name) ||
      (config.tools?.internet === true &&
        (tool.function.name === 'web_search' && requiresWebSearch ||
          tool.function.name === 'web_fetch' && requiresWebFetch))) &&
    (tool.function.name !== 'run_test' || verification.run && config.tools?.run_test !== false) &&
    (!sliceReadsOnly || !['list_dir', 'glob_files', 'search_text'].includes(tool.function.name)) &&
    (!boundedTask || ['read_file', 'write_file', 'edit_file', 'run_test', 'web_search', 'web_fetch'].includes(tool.function.name))).map((tool) =>
    !boundedTask || tool.function.name === 'run_test' || tool.function.name === 'web_search' || tool.function.name === 'web_fetch' ? tool : {
      ...tool, function: { ...tool.function, parameters: { ...tool.function.parameters,
        properties: { ...tool.function.parameters.properties,
          path: tool.function.name === 'read_file' && !docsOnly
            ? { type: 'string', description: 'Worktree-root-relative path: TASK.md, an allowed file, or its direct local imports. Never use absolute paths or .. components.' }
            : { type: 'string', enum: tool.function.name === 'read_file'
              ? ['TASK.md', singleAllowedFile] : [singleAllowedFile] },
        },
      } },
    });
  const offeredTools = new Set(definitions.map((tool) => tool.function.name));
  await onEvent?.({ type: 'toolset', tools: [...offeredTools] });
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
  const readCache = new Map();
  const usageDenials = new Map();
  progress.testRepairs = 0;
  progress.testRepairBudget = repairBudget;
  progress.repairFiles = [];
  progress.regressionFiles = [];
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
    // repair_files is the cumulative write grant; failing_files is what fails in this run.
    const failingFiles = tests.failing_files ?? tests.repair_files ?? [];
    for (const file of tests.regression_files ?? []) {
      if (!progress.regressionFiles.includes(file)) progress.regressionFiles.push(file);
    }
    // Regressions this change caused outside Allowed Files are repairable; only proven non-regressions are excused.
    const excused = (file) => !parsedTask.files_allowed.includes(file) && !progress.regressionFiles.includes(file);
    progress.repairFiles = failingFiles.filter((file) => !excused(file));
    const outside = failingFiles.filter(excused);
    const regressions = progress.repairFiles.filter((file) => progress.regressionFiles.includes(file));
    const redact = (text) => redactEvidence(String(text ?? ''), { env, apiKeyEnv: config.llm.api_key_env });
    const output = [testFailureEvidence(redact(tests.stdout), 3072), redact(tests.stderr).slice(-1024)]
      .filter(Boolean).join('\n');
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
    const fingerprint = failureFingerprint(output);
    const failCount = failingTestCount(tests.stdout);
    progress.seenFailures ??= [];
    // Any earlier failure counts, so oscillating between two failures is a repeat, not progress.
    const repeated = progress.seenFailures.includes(fingerprint);
    const worse = failCount !== undefined && progress.failCount !== undefined && failCount > progress.failCount;
    const progressed = progress.testRepairs > 0 && !repeated && !worse;
    if (!repeated) progress.seenFailures.push(fingerprint);
    if (failCount !== undefined) progress.failCount = failCount;
    if (repeated) {
      progress.repeatedFailures = (progress.repeatedFailures ?? 0) + 1;
      if (progress.repeatedFailures >= 2) {
        progress.repairRepeated = true;
        throw new Error(`${failure}\nTest repair stalled: an earlier failure repeated after ${progress.testRepairs} repairs`);
      }
    }
    if (progress.testRepairs === repairBudget && progressed && repairBudget < testRepairCap) {
      const contextUsed = usages.at(-1)?.prompt_tokens;
      const contextMax = config.llm.context_max;
      if (contextMax > 0 && contextUsed >= contextMax * repairContextShare) {
        // Progressing but crowded: a fresh context with the edits kept is cheaper than resending this history.
        progress.contextHandoff = true;
        throw new Error(`${failure}\nTest repair handoff: context ${contextUsed} of ${contextMax} tokens after ` +
          `${progress.testRepairs} progressing repairs`);
      }
      repairBudget += 1;
      progress.testRepairBudget = repairBudget;
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
      `Repair ${progress.testRepairs} of ${repairBudget}` +
      (repairBudget < testRepairCap ? ' (each repair that changes the failure without adding failing tests earns another)' : '') +
      '. Repair only TASK-allowed files' +
      (progress.repairFiles.length ? `: ${progress.repairFiles.join(', ')}` : '') +
      '. ' + (repeated ? 'This failure already occurred after an earlier repair, so that approach did not work. ' +
        'Take a materially different approach; one more repeat stops this context. ' : '') +
      (regressions.length ? `These tests pass at the base commit, so this change broke them: ${regressions.join(', ')}. ` +
        'Decide from TASK.md whether the new behavior is intended (update the test to it) or the implementation ' +
        'regressed (fix the implementation). ' : '') +
      'Do not edit any other failing test outside Allowed Files. Report it as pre-existing. ' +
      'Fix the exact reported root cause and ensure every identifier introduced by the change is defined in its scope. ' +
      (progress.testRepairs > 1 ? 'Earlier repairs did not make the suite pass, so change strategy: decide from TASK.md ' +
        'whether the failing assertion or the implementation is wrong and change only that side; do not alternate ' +
        'between editing a test and its implementation to chase the same assertion. ' : '') +
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
      if (error instanceof ChatError && !(error instanceof UnsupportedFinishReasonError) &&
          finalSummaryOnly && checksPassedAfterWrite) {
        acceptedLateLength = true;
        usages.push(chat.lastUsage);
        response = { finish_reason: 'stop', message: { role: 'assistant', content: '' }, usage: chat.lastUsage };
      } else {
      throw error;
      }
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
      if (error instanceof UnknownCoderTool && !progress.unknownToolCorrected) {
        progress.unknownToolCorrected = true;
        needsTools = true;
        await onEvent?.({ type: 'tool-result', name: error.toolName, status: 'denied' });
        messages.push({ role: 'user', content: `Tool ${error.toolName} does not exist. Offered tools: ` +
          `${[...offeredTools].join(', ')}. Continue with one offered tool call, or summarize the blocker.` });
        continue;
      }
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
      if (progress.testRepairs === 0) {
        const exploring = calls.every((call) =>
          ['read_file', 'list_dir', 'search_text', 'web_search', 'web_fetch'].includes(call.function.name));
        const turnBudgetSpent = attemptTurns === config.seat.turn_budget + Number(repaired);
        const explorationSpent = attemptTurns >= explorationBudget;
        if (exploring && progress.writeForced) {
          if (progress.writeForcedAgain) {
            throw new Error('Coder continued exploring after the bounded exploration budget was spent');
          }
          progress.writeForcedAgain = true;
          for (const call of calls) {
            messages.push({ role: 'tool', tool_call_id: call.id,
              content: 'Denied: reads are closed. Use the content already in this conversation.' });
            await onEvent?.({ type: 'tool-result', name: call.function.name, status: 'denied' });
          }
          messages.push({ role: 'user', content: 'Final notice: the next response must be write_file or edit_file on an allowed file. ' +
            'Make the smallest complete change from what you already read; another read ends the run.' });
          continue;
        }
        if (exploring && (turnBudgetSpent || explorationSpent)) {
          progress.writeForced = true;
          attemptTurns = 0;
          for (const call of calls) {
            messages.push({ role: 'tool', tool_call_id: call.id,
              content: 'Exploration budget is spent. Next response must be write_file for the allowed files only.' });
          }
          messages.push({ role: 'user', content: 'Stop reading and searching. Write the allowed files now, then summarize.' });
          continue;
        }
        if (turnBudgetSpent) {
          throw new Error(`Coder turn budget (${config.seat.turn_budget}) exhausted before a summary`);
        }
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
            ['write_file', 'edit_file'].includes(pending.function.name) &&
              pending.args.path === singleAllowedFile);
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
            !coversTestFile(verification.update, call.args.path) &&
            !progress.repairFiles.includes(String(call.args.path).replaceAll('\\', '/'))) {
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
          // "Repeated" means the same call failing the same way, not unrelated misses separated by progress.
          const denialKey = `${call.function.name}\0${error instanceof Error ? error.message : ''}\0${JSON.stringify(call.args)}`;
          const repeated = error instanceof ToolUsageError &&
            (usageDenials.set(denialKey, (usageDenials.get(denialKey) ?? 0) + 1).get(denialKey) > maxRepeatedDenials);
          if (error instanceof ToolAccessError && (!(error instanceof ToolUsageError) || repeated)) {
            await onEvent?.({ type: 'tool-result', name: call.function.name, status: 'denied' });
            throw repeated ? new ToolAccessError(`${error.message.split('\n')[0]} (repeated after ${maxRepeatedDenials} denials)`) : error;
          }
          messages.push({ role: 'tool', tool_call_id: call.id,
            content: redactEvidence(`Denied: ${error instanceof Error ? error.message : 'tool failed'}. Continue with an allowed action or summarize the blocker.`,
              { env, apiKeyEnv: config.llm.api_key_env }) });
          await onEvent?.({ type: 'tool-result', name: call.function.name, status: 'denied' });
          continue;
        }
        const shown = call.function.name === 'run_test' && result && typeof result === 'object' && result.exit_code !== 0
          ? { ...result, stdout: testFailureEvidence(result.stdout, 8000), stderr: String(result.stderr ?? '').slice(-2000) }
          : result;
        const content = redactEvidence(typeof shown === 'string' ? shown : JSON.stringify(shown), {
          env, apiKeyEnv: config.llm.api_key_env,
        });
        const readKey = call.function.name === 'read_file' ? String(call.args.path ?? '').replaceAll('\\', '/') : undefined;
        const unchanged = readKey !== undefined && readCache.get(readKey) === content;
        if (readKey !== undefined) readCache.set(readKey, content);
        messages.push({ role: 'tool', tool_call_id: call.id, content: unchanged
          ? `Unchanged since your earlier read_file of ${readKey} in this session; reuse that content instead of rereading.`
          : content });
        if (call.function.name === 'web_search') webSearchDone = true;
        if (call.function.name === 'web_fetch') webFetchDone = true;
        if (['write_file', 'edit_file'].includes(call.function.name)) usageDenials.clear();
        if (['write_file', 'edit_file'].includes(call.function.name) &&
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
        const testsSkipped = !verification.run || taskSkipsTests(context.task);
        const tests = testsSkipped ? undefined : await tools.run_test({}, { full: true });
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
      : testsSkipped && progress.testRepairs === 0 ? undefined : await tools.run_test({}, { full: true });
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
    const reasons = excellence.reasons.map((reason) => redactEvidence(reason, {
      env, apiKeyEnv: config.llm.api_key_env,
    }));
    const noDiff = reasons.find((reason) => reason.includes('must produce an application diff'));
    if (noDiff && reasons.length === 1 && !progress.noProgressRepairUsed) {
      progress.noProgressRepairUsed = true;
      attemptTurns = 0;
      finalSummaryOnly = false;
      checksPassedAfterWrite = false;
      needsTools = false;
      messages.push({ role: 'assistant', content: summary });
      messages.push({ role: 'user', content: `${noDiff}\n` +
        'One no-progress correction is allowed. Read the allowed file and its direct imports using root-relative paths. ' +
        'Implement the missing Ask within Allowed Files, preserve existing behavior, rerun the required tests, and summarize. ' +
        'Do not invent a fixture-only stand-in for the public behavior. If blocked, report the blocker rather than claim success.' });
      continue;
    }
    const secretReasons = reasons.filter((reason) => reason.startsWith('Secret material detected'));
    const substanceReasons = reasons.filter((reason) => reason.startsWith('Test substance:'));
    const repairable = secretReasons.length + substanceReasons.length === reasons.length;
    if (repairable && reasons.length && ((secretReasons.length && !progress.secretRepairUsed) ||
        (substanceReasons.length && !progress.substanceRepairUsed))) {
      if (secretReasons.length) progress.secretRepairUsed = true;
      if (substanceReasons.length) progress.substanceRepairUsed = true;
      attemptTurns = 0;
      finalSummaryOnly = false;
      checksPassedAfterWrite = false;
      needsTools = false;
      messages.push({ role: 'assistant', content: summary });
      messages.push({ role: 'user', content: [
        ...secretReasons, ...substanceReasons,
        ...(secretReasons.length ? ['One secret-material correction is allowed. Credential-shaped literals are rejected even in test fixtures. ' +
          `${sentinelGuidance} Replace every such literal in Allowed Files and keep the assertions.`] : []),
        ...(substanceReasons.length ? ['One test-substance correction is allowed. Add at least one new test block. Each new test must call the imported app function under test ' +
          '(directly or via an existing helper) and assert on its output. Pass any seeded sentinel into that call ' +
          '(argument, config object, or process.env) before asserting it is absent from the output; never build, strip, and inspect your own object. ' +
          'If a module you wanted is outside read scope, use exports the test file already imports or the listed public seams instead of stopping.'] : []),
        'Rerun the required tests, and summarize.',
      ].join('\n') });
      continue;
    }
    if (deterministic) throw new Error(`Deterministic fallback failed excellence: ${excellence.reasons[0]}`);
    // Out-of-scope files (typically scratch probes) are a correctable mistake, not a failed run;
    // protected paths stay a hard stop.
    const outOfScope = reasons.filter((reason) => reason.startsWith('Diff path is outside TASK.md allowed paths: '));
    if (outOfScope.length && definitions.some((tool) => tool.function.name === 'delete_file') &&
        outOfScope.length === reasons.filter((reason) => !reason.startsWith('node --test failed (exit ')).length &&
        (progress.scopeCorrections ?? 0) < 2) {
      progress.scopeCorrections = (progress.scopeCorrections ?? 0) + 1;
      attemptTurns = 0;
      finalSummaryOnly = false;
      checksPassedAfterWrite = false;
      needsTools = false;
      messages.push({ role: 'assistant', content: summary });
      messages.push({ role: 'user', content: [
        ...outOfScope,
        'These files are in the diff but outside TASK.md Allowed Files. Delete scratch, probe, or diagnostic files ' +
        'with delete_file. If the change genuinely needs one, rewrite it with write_file so it is recorded as a scope ' +
        'expansion and justify it in your summary (the reviewer judges it). Rerun the required tests, then summarize.',
      ].join('\n') });
      continue;
    }
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
    // Unresolved test substance is a correctable quality defect, so a fresh perspective may fix it.
    if (unsafe?.startsWith('Test substance:') && reasons.every((reason) => reason.startsWith('Test substance:') ||
      reason.startsWith('node --test failed (exit '))) progress.substanceUnresolved = true;
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
