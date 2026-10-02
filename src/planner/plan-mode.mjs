import { createBuiltinChat } from '../lib/llm.mjs';
import { mergeUsage } from '../metrics/run.mjs';
import { buildPlan, planFromTask, planStub } from './stub.mjs';
import { parseTaskDocument } from './task.mjs';
import { planArtifactFiles, toolDefinitions } from '../runtime/tools.mjs';
import { throwIfCancelled } from '../runtime/cancel.mjs';

export const planModeTools = toolDefinitions.filter(({ function: tool }) =>
  ['read_file', 'list_dir', 'search_text', 'write_file'].includes(tool.name)).map((tool) =>
  tool.function.name === 'write_file' ? { ...tool, function: { ...tool.function,
    description: 'Write only the root PLAN.md. Product files, TASK.md, and RECIPE.yml are forbidden.',
    parameters: { ...tool.function.parameters, properties: { ...tool.function.parameters.properties,
      path: { type: 'string', enum: [...planArtifactFiles] }, content: { type: 'string', maxLength: 65536 } } },
  } } : tool);

export function validatedPlanTask(plan, ask, title, lockedModel) {
  const task = plan.replace(/^# Plan:/m, '# Task:');
  parseTaskDocument(task, { expectedAsk: ask, issueTitle: title });
  const metadata = planFromTask(task, ask, { issueTitle: title, issueBody: ask });
  if (lockedModel && metadata.model && metadata.model !== lockedModel) {
    throw new Error('The routed plan must keep the selected fleet model');
  }
  return task;
}

export async function planSlice(ask, {
  config, reference, title, metadata, lockedModel, tools, fetchImpl, env, vault, onEvent, onResponse, retryCommand, signal,
}) {
  if (!config.llm.base_url) {
    return { plan: planStub(ask, { reference, title, metadata }).task.replace(/^# Task:/m, '# Plan:'),
      mode: 'stub', turns: 0, usage: null, response: null };
  }
  const chat = createBuiltinChat(config, { fetchImpl, env, vault, onEvent, retryCommand, signal });
  const messages = [
    { role: 'system', content: 'You are the read-only plan-mode planner. Explore with read_file, list_dir, or search_text. ' +
      'You may write only PLAN.md, never TASK.md, RECIPE.yml, product code, tests, or publishing operations. ' +
      'PLAN.md must contain a title, Original Ask, Acceptance checks, explicit Files allowed, and task metadata. ' +
      'Alternatively return JSON with title, acceptance_checks, files_allowed, and optional difficulty, estimate_min, task_class, model. ' +
      'Stay inside the files named by the human Ask. The human must accept before coding.' },
    { role: 'user', content: ask },
  ];
  const usage = [];
  const ids = new Set();
  for (let turn = 1; turn <= config.planner.turn_budget; turn += 1) {
    throwIfCancelled(signal);
    let response;
    try { response = await chat({ messages, tools: planModeTools }); }
    catch (error) { if (chat.lastResponse) onResponse?.(chat.lastResponse); throw error; }
    onResponse?.(chat.lastResponse);
    usage.push(response.usage);
    const message = response.message;
    if (message.tool_calls?.length) {
      if (response.finish_reason === 'stop') throw new Error('Plan-mode planner stopped while requesting tools');
      messages.push({ role: 'assistant', content: message.content ?? null, tool_calls: message.tool_calls });
      for (const call of message.tool_calls) {
        if (call.type !== 'function' || !planModeTools.some((tool) => tool.function.name === call.function?.name) ||
            typeof call.id !== 'string' || !call.id || ids.has(call.id) || typeof call.function.arguments !== 'string') {
          throw new Error('Plan-mode planner requested an invalid or unavailable tool');
        }
        ids.add(call.id);
        let args;
        try { args = JSON.parse(call.function.arguments); }
        catch { throw new Error('Plan-mode tool arguments must be JSON'); }
        if (call.function.name === 'write_file' && args?.path === 'PLAN.md') {
          validatedPlanTask(args.content, ask, title, lockedModel);
        }
        const result = await tools[call.function.name](args);
        messages.push({ role: 'tool', tool_call_id: call.id, content: typeof result === 'string' ? result : JSON.stringify(result) });
        if (call.function.name === 'write_file') return { plan: args.content, mode: 'llm',
          turns: turn, usage: mergeUsage(...usage), response: chat.lastResponse };
      }
      continue;
    }
    let description;
    try { description = JSON.parse(message.content); }
    catch { throw new Error('Plan-mode planner must return a complete JSON plan or write PLAN.md'); }
    const allowedKeys = ['title', 'acceptance_checks', 'files_allowed', 'difficulty', 'estimate_min', 'task_class', 'model'];
    if (!description || typeof description !== 'object' || Array.isArray(description) ||
        Object.keys(description).some((key) => !allowedKeys.includes(key))) throw new Error('Invalid plan-mode description');
    const draft = buildPlan(ask, { reference, title: description.title, acceptanceChecks: description.acceptance_checks,
      filesAllowed: description.files_allowed, metadata: { ...metadata,
        ...Object.fromEntries(['difficulty', 'estimate_min', 'task_class', 'model'].filter((key) =>
          description[key] !== undefined).map((key) => [key, description[key]])) } });
    const plan = draft.task.replace(/^# Task:/m, '# Plan:');
    validatedPlanTask(plan, ask, title, lockedModel);
    return { plan, mode: 'llm', turns: turn, usage: mergeUsage(...usage), response: chat.lastResponse };
  }
  throw new Error(`Plan-mode turn budget (${config.planner.turn_budget}) exhausted; no coder ran`);
}
