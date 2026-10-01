import { createBuiltinChat } from '../lib/llm.mjs';
import { mergeUsage } from '../metrics/run.mjs';
import { toolDefinitions } from './tools.mjs';
import { redactEvidence, taskSkipsTests } from './excellence.mjs';
import { parseTaskDocument } from '../planner/task.mjs';
import { applyReadmeStatus } from './readme-status.mjs';

class MalformedCoderTools extends Error {}

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

async function executeLoop({ config, context, tools, fetchImpl, env, vault, verify, onEvent }, progress) {
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

  const messages = [
    { role: 'system', content: context.pack },
    { role: 'user', content: 'Complete this task using only the offered tools. ' +
      'Read RESEARCH.md for the pre-edit inventory and gaps. ' +
      'Do not claim acceptance checks passed without evidence. ' +
      'Finish with a concise summary of changes, test results, and blockers.' },
  ];
  const definitions = toolDefinitions.filter((tool) => config.seat.tools.includes(tool.function.name) &&
    (tool.function.name !== 'run_test' || config.tools?.run_test !== false));
  const offeredTools = new Set(definitions.map((tool) => tool.function.name));
  const chat = createBuiltinChat(config, { fetchImpl, env, vault, onEvent });
  const usages = [];
  const ids = new Set();
  let repaired = false;
  let needsTools = false;
  await onEvent?.({ type: 'implementation', path: 'model' });
  for (let turn = 1; turn <= config.seat.turn_budget + Number(repaired); turn += 1) {
    progress.turns = turn;
    progress.usage = null;
    const response = await chat({ messages, tools: definitions });
    progress.response = chat.lastResponse;
    usages.push(response.usage);
    progress.usage = mergeUsage(...usages);
    const { message, finish_reason: finishReason } = response;
    if (!message || typeof message !== 'object' || !['stop', 'tool_calls', undefined, null]
      .includes(finishReason)) {
      throw new Error('LLM coder returned an unsupported chat response');
    }
    let calls;
    let deterministic;
    try {
      calls = decodeCalls(message, offeredTools, ids, turn);
      if ((needsTools || finishReason === 'tool_calls') && !calls.length) {
        throw new MalformedCoderTools('Coder repair did not emit tool calls');
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
      if (turn === config.seat.turn_budget + Number(repaired)) {
        throw new Error(`Coder turn budget (${config.seat.turn_budget}) exhausted before a summary`);
      }
      if (finishReason === 'stop') throw new Error('LLM coder stopped while requesting tools');
      calls.forEach((call) => ids.add(call.id));
      messages.push({ role: 'assistant', content: message.content ?? null,
        tool_calls: calls.map(({ args: _args, ...call }) => call) });
      for (const call of calls) {
        let result;
        try {
          result = await tools[call.function.name](call.args);
        } catch (error) {
          if (!(error instanceof Error)) throw error;
          if (error.code === 'ROSTER_RUN_LOG') throw error;
          result = { error: error.message };
        }
        messages.push({
          role: 'tool', tool_call_id: call.id,
          content: typeof result === 'string' ? result : JSON.stringify(result),
        });
      }
      continue;
    }
    if (!deterministic && (finishReason === 'tool_calls' || typeof message.content !== 'string' ||
        !message.content.trim())) {
      throw new Error('LLM coder did not return a final summary');
    }
    const usage = mergeUsage(...usages);
    const testsSkipped = taskSkipsTests(context.task);
    const tests = testsSkipped ? undefined : await tools.run_test({});
    progress.tests = tests;
    const summary = deterministic ?? message.content.trim();
    const result = {
      mode: 'llm', model: config.llm.model, summary, usage, turns: turn, tests, testsSkipped,
      implementationPath: progress.implementationPath ?? 'model',
    };
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
    const output = redactEvidence([tests.stdout, tests.stderr].filter(Boolean).join('\n'), {
      env, apiKeyEnv: config.llm.api_key_env,
    }).slice(0, 4096);
    const failure = `Final node --test failed (exit ${tests.exit_code}):\n${output}`;
    if (turn === config.seat.turn_budget + Number(repaired)) {
      throw new Error(`${failure}\nCoder turn budget (${config.seat.turn_budget}) exhausted`);
    }
    messages.push({ role: 'assistant', content: summary });
    messages.push({ role: 'user', content: `${failure}\nFix the failing tests using only the offered tools, ` +
      'then provide a new summary. No change is verified yet.' });
  }

  throw new Error(`Coder turn budget (${config.seat.turn_budget}) exhausted`);
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
    return { ...progress, summary: 'Coder execution stopped before a verified result.', error };
  }
}
