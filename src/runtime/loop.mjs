import { promises as fs } from 'node:fs';
import path from 'node:path';
import { createBuiltinChat } from '../lib/llm.mjs';
import { mergeUsage } from '../metrics/run.mjs';
import { toolDefinitions } from './tools.mjs';

function stubSummary(task) {
  const title = /^# Task: (.+)$/m.exec(task)?.[1];
  const checks = /^## Acceptance checks\n((?:- .+\n)+)/m.exec(task)?.[1];
  if (!title || !checks) throw new Error('TASK.md needs a title and acceptance checks');
  return `Task: ${title}\n\nAcceptance checks:\n${checks.trimEnd()}\n\n` +
    'Deterministic stub only: no implementation or tests were run. Configure llm.base_url to run a coder.';
}

async function saveResult(worktree, body) {
  const file = path.join(worktree, 'RESULT.md');
  await fs.writeFile(file, `# Result\n\n${body}\n`, { encoding: 'utf8', flag: 'wx' });
  return file;
}

export async function runLoop({ config, context, tools, worktree, fetchImpl, env, vault }) {
  if (!config.llm.base_url) {
    const summary = stubSummary(context.task);
    return {
      mode: 'stub', summary, usage: null, turns: 0,
      resultPath: await saveResult(worktree, summary),
    };
  }

  const messages = [
    { role: 'system', content: context.pack },
    { role: 'user', content: 'Complete this task using only the offered tools. ' +
      'Do not claim acceptance checks passed without evidence. ' +
      'Finish with a concise summary of changes, test results, and blockers.' },
  ];
  const definitions = toolDefinitions.filter((tool) => config.seat.tools.includes(tool.function.name));
  const offeredTools = new Set(definitions.map((tool) => tool.function.name));
  const chat = createBuiltinChat(config, { fetchImpl, env, vault });
  const usages = [];
  const ids = new Set();
  for (let turn = 1; turn <= config.seat.turn_budget; turn += 1) {
    const response = await chat({ messages, tools: definitions });
    usages.push(response.usage);
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
    const tests = await tools.run_test({});
    if (tests.exit_code !== 0) {
      throw new Error(`Final node --test failed (exit ${tests.exit_code}):\n` +
        `${tests.stderr || tests.stdout}`.slice(0, 4096));
    }
    const summary = message.content.trim();
    return {
      mode: 'llm', summary, usage, turns: turn, tests,
      resultPath: await saveResult(worktree, `${summary}\n\n## Verification\n- node --test exited 0`),
    };
  }
  throw new Error(`Coder turn budget (${config.seat.turn_budget}) exhausted`);
}
