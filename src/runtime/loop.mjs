import { createBuiltinChat } from '../lib/llm.mjs';
import { mergeUsage } from '../metrics/run.mjs';
import { toolDefinitions } from './tools.mjs';
import { redactEvidence, taskSkipsTests } from './excellence.mjs';

function stubSummary(task) {
  const title = /^# Task: (.+)$/m.exec(task)?.[1];
  const checks = /^## Acceptance checks\n((?:- .+\n)+)/m.exec(task)?.[1];
  if (!title || !checks) throw new Error('TASK.md needs a title and acceptance checks');
  return `Task: ${title}\n\nAcceptance checks:\n${checks.trimEnd()}\n\n` +
    'Deterministic stub only: no implementation or tests were run. Configure llm.base_url to run a coder.';
}

async function executeLoop({ config, context, tools, fetchImpl, env, vault, verify }, progress) {
  if (!config.llm.base_url) {
    const summary = stubSummary(context.task);
    return {
      mode: 'stub', summary, usage: null, turns: 0,
    };
  }
  if (typeof verify !== 'function') {
    throw new TypeError('Configured coder requires an excellence verifier');
  }

  const messages = [
    { role: 'system', content: context.pack },
    { role: 'user', content: 'Complete this task using only the offered tools. ' +
      'Read RESEARCH.md for the pre-edit inventory and gaps. ' +
      'Do not claim acceptance checks passed without evidence. ' +
      'Finish with a concise summary of changes, test results, and blockers.' },
  ];
  const definitions = toolDefinitions.filter((tool) => config.seat.tools.includes(tool.function.name));
  const offeredTools = new Set(definitions.map((tool) => tool.function.name));
  const chat = createBuiltinChat(config, { fetchImpl, env, vault });
  const usages = [];
  const ids = new Set();
  for (let turn = 1; turn <= config.seat.turn_budget; turn += 1) {
    progress.turns = turn;
    progress.usage = null;
    const response = await chat({ messages, tools: definitions });
    usages.push(response.usage);
    progress.usage = mergeUsage(...usages);
    const { message, finish_reason: finishReason } = response;
    if (!message || typeof message !== 'object' || !['stop', 'tool_calls', undefined, null]
      .includes(finishReason)) {
      throw new Error('LLM coder returned an unsupported chat response');
    }
    if (message.tool_calls !== undefined && !Array.isArray(message.tool_calls)) {
      throw new Error('LLM coder returned malformed tool calls');
    }
    if (message.tool_calls?.length) {
      if (turn === config.seat.turn_budget) {
        throw new Error(`Coder turn budget (${config.seat.turn_budget}) exhausted before a summary`);
      }
      if (finishReason === 'stop') throw new Error('LLM coder stopped while requesting tools');
      const calls = message.tool_calls.map((call) => {
        if (typeof call?.id !== 'string' || !call.id || ids.has(call.id) ||
            call.type !== 'function' || !offeredTools.has(call.function?.name) ||
            typeof call.function.arguments !== 'string') {
          throw new Error('LLM coder requested an invalid or unavailable tool');
        }
        ids.add(call.id);
        return { id: call.id, type: 'function',
          function: { name: call.function.name, arguments: call.function.arguments } };
      });
      messages.push({ role: 'assistant', content: message.content ?? null, tool_calls: calls });
      for (const call of calls) {
        let result;
        try {
          const args = JSON.parse(call.function.arguments);
          result = await tools[call.function.name](args);
        } catch (error) {
          if (!(error instanceof Error)) throw error;
          result = { error: error.message };
        }
        messages.push({
          role: 'tool', tool_call_id: call.id,
          content: typeof result === 'string' ? result : JSON.stringify(result),
        });
      }
      continue;
    }
    if (finishReason === 'tool_calls' || typeof message.content !== 'string' ||
        !message.content.trim()) {
      throw new Error('LLM coder did not return a final summary');
    }
    const usage = mergeUsage(...usages);
    const testsSkipped = taskSkipsTests(context.task);
    const tests = testsSkipped ? undefined : await tools.run_test({});
    progress.tests = tests;
    const summary = message.content.trim();
    const result = {
      mode: 'llm', model: config.llm.model, summary, usage, turns: turn, tests, testsSkipped,
    };
    const excellence = await verify(result);
    if (typeof excellence?.pass !== 'boolean' || !Array.isArray(excellence.reasons)) {
      throw new TypeError('Coder excellence verifier returned an invalid report');
    }
    if (excellence.pass && (!tests || tests.exit_code === 0)) return result;

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
    if (turn === config.seat.turn_budget) {
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
    turns: 0, usage: null,
  };
  try {
    const result = await executeLoop(options, progress);
    return { ...progress, ...result };
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    return { ...progress, summary: 'Coder execution stopped before a verified result.', error };
  }
}
