import { RunCancelledError, throwIfCancelled } from '../../src/runtime/cancel.mjs';
import { createTools, toolDefinitions } from '../../src/runtime/tools.mjs';

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const text = (value) => typeof value === 'string' && value.trim() && !/[\0\r\n]/.test(value);

function exact(value, required, optional = []) {
  if (!isObject(value) || required.some((key) => !Object.hasOwn(value, key)) ||
      Object.keys(value).some((key) => ![...required, ...optional].includes(key))) {
    throw new TypeError('Offline fixture has missing or unknown fields');
  }
}

// This is an inert process-design descriptor, not a CLI sandbox or launch implementation.
export function offlineInferenceContext(parentEnv = {}) {
  if (!isObject(parentEnv)) throw new TypeError('Parent environment must be an object');
  return Object.freeze({
    env: Object.freeze({}),
    useLoggedInUser: false,
    executable: null,
    ghFallback: 'unavailable-no-process',
  });
}

function validate(request, fixture) {
  exact(request, ['model', 'messages', 'tools']);
  if (!text(request.model) || !Array.isArray(request.messages) || !request.messages.length ||
      request.messages.some((message) => !isObject(message) ||
        Object.keys(message).some((key) => !['role', 'content'].includes(key)) ||
        !['system', 'user'].includes(message.role) || typeof message.content !== 'string') ||
      !Array.isArray(request.tools) || request.tools.length > 1 ||
      request.tools.some((tool) => JSON.stringify(tool) !== JSON.stringify(
        toolDefinitions.find((definition) => definition.function.name === 'read_file')))) {
    throw new TypeError('Offline turn requires bounded text messages and only the Roster read_file tool');
  }
  exact(fixture, ['events'], ['delayMs']);
  if (!Array.isArray(fixture.events) || fixture.events.length < 2 || fixture.events.length > 4 ||
      fixture.delayMs !== undefined && (!Number.isSafeInteger(fixture.delayMs) ||
        fixture.delayMs < 0 || fixture.delayMs > 1000) ||
      Buffer.byteLength(JSON.stringify({ request, fixture }), 'utf8') > 65_536) {
    throw new TypeError('Offline fixture exceeds its event, delay or byte budget');
  }
  let usage;
  let call;
  let content;
  let idle = false;
  for (const event of fixture.events) {
    exact(event, ['type', 'data']);
    if (idle) throw new TypeError('Offline fixture has events after idle');
    switch (event.type) {
      case 'assistant.usage':
        if (usage !== undefined || content !== undefined) throw new TypeError('Duplicate or late usage');
        exact(event.data, [], ['model', 'inputTokens', 'outputTokens', 'cost']);
        if (event.data.model !== undefined && !text(event.data.model) ||
            ['inputTokens', 'outputTokens'].some((key) => event.data[key] !== undefined &&
              (!Number.isSafeInteger(event.data[key]) || event.data[key] < 0)) ||
            event.data.cost !== undefined && (!Number.isFinite(event.data.cost) || event.data.cost < 0)) {
          throw new TypeError('Offline fixture usage is invalid');
        }
        usage = event.data;
        break;
      case 'fixture.tool_request':
        if (call || content !== undefined) throw new TypeError('Offline turn permits one tool request before its message');
        exact(event.data, ['id', 'name', 'arguments']);
        if (!text(event.data.id) || event.data.name !== 'read_file' || request.tools.length !== 1 ||
            !isObject(event.data.arguments)) {
          throw new TypeError('Offline fixture requested an unavailable or malformed tool');
        }
        exact(event.data.arguments, ['path'], ['offset', 'max_lines']);
        if (!text(event.data.arguments.path) || ['offset', 'max_lines'].some((key) =>
          event.data.arguments[key] !== undefined && (!Number.isSafeInteger(event.data.arguments[key]) ||
            event.data.arguments[key] < 1))) {
          throw new TypeError('Offline fixture tool arguments are invalid');
        }
        call = event.data;
        break;
      case 'assistant.message':
        exact(event.data, ['content']);
        if (content !== undefined || typeof event.data.content !== 'string') {
          throw new TypeError('Offline fixture requires one assistant message');
        }
        content = event.data.content;
        break;
      case 'session.idle':
        exact(event.data, []);
        if (content === undefined) throw new TypeError('Offline fixture idle preceded its message');
        idle = true;
        break;
      default:
        throw new TypeError('Offline fixture event type is unavailable');
    }
  }
  if (!idle || content === undefined || !content.trim() && !call) {
    throw new TypeError('Offline fixture did not complete a turn');
  }
  return { usage, call, content };
}

function wait(milliseconds, signal) {
  return new Promise((resolve, reject) => {
    const abort = () => {
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      reject(signal.reason);
    };
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', abort);
      resolve();
    }, milliseconds);
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) abort();
  });
}

// Synthetic transcript only: no SDK, subprocess, network, auth or publication capability.
export async function runOfflineTurn({ request, fixture, worktree, allowedFiles,
  env = {}, signal, deadlineMs = 1000, lifecycle = {} }) {
  if (!Number.isSafeInteger(deadlineMs) || deadlineMs < 1 || deadlineMs > 1000) {
    throw new TypeError('Offline deadline must be 1-1000 milliseconds');
  }
  ({ request, fixture } = structuredClone({ request, fixture }));
  if (!Array.isArray(allowedFiles) || !allowedFiles.length ||
      allowedFiles.some((file) => !text(file) || file.includes('*'))) {
    throw new TypeError('Offline tool scope requires explicit file names');
  }
  allowedFiles = [...allowedFiles];
  const decoded = validate(request, fixture);
  const context = offlineInferenceContext(env);
  throwIfCancelled(signal);
  const controller = new AbortController();
  const cancel = () => controller.abort(new RunCancelledError());
  signal?.addEventListener('abort', cancel, { once: true });
  if (signal?.aborted) cancel();
  const timer = setTimeout(() => controller.abort(Object.assign(
    new Error('Offline turn deadline exceeded'), { code: 'ROSTER_TASK_TIMEOUT' })), deadlineMs);
  const events = [{ type: 'toolset', tools: request.tools.map((tool) => tool.function.name) }];
  lifecycle.active = true;
  lifecycle.turns = 1;
  lifecycle.aborted = false;
  try {
    await wait(fixture.delayMs ?? 0, controller.signal);
    throwIfCancelled(controller.signal);
    const toolMessages = [];
    if (decoded.call) {
      const tools = await createTools({ worktree, allowedFiles, sliceReadsOnly: true,
        scopeExpansion: 0, allowRunTest: false, allowInternet: false,
        runCommand: () => { throw new Error('Offline fixture cannot execute a command'); },
        fetchImpl: () => { throw new Error('Offline fixture cannot access the network'); },
        env: context.env, signal: controller.signal });
      throwIfCancelled(controller.signal);
      const { id, name, arguments: args } = decoded.call;
      try {
        const result = await tools.read_file(args);
        throwIfCancelled(controller.signal);
        toolMessages.push({ role: 'tool', tool_call_id: id, content: result });
        events.push({ type: 'tool-result', name, status: 'ok' });
      } catch (error) {
        events.push({ type: 'tool-result', name, status: 'denied' });
        throw error;
      }
    }
    const { usage, call, content } = decoded;
    const model = usage?.model ?? null;
    const unknown = ['model', 'inputTokens', 'outputTokens', 'cost'].filter((key) => usage?.[key] === undefined);
    const finishReason = call ? 'tool_calls' : 'stop';
    events.push({ type: 'completion', reason: finishReason });
    return {
      mode: 'offline-fixture',
      response: {
        message: { role: 'assistant', content, ...(call ? { tool_calls: [{
          id: call.id, type: 'function',
          function: { name: call.name, arguments: JSON.stringify(call.arguments) },
        }] } : {}) },
        model,
        usage: usage && (usage.inputTokens !== undefined || usage.outputTokens !== undefined) ? {
          ...(usage.inputTokens === undefined ? {} : { prompt_tokens: usage.inputTokens }),
          ...(usage.outputTokens === undefined ? {} : { completion_tokens: usage.outputTokens }),
        } : null,
        finish_reason: finishReason,
      },
      toolMessages,
      events,
      provenance: { provider: 'offline-fixture', model, unknown,
        reportedCost: usage?.cost ?? null, publishable: false },
      context,
    };
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', cancel);
    lifecycle.aborted = controller.signal.aborted;
    lifecycle.active = false;
    lifecycle.disconnected = true;
    lifecycle.stopped = true;
  }
}
