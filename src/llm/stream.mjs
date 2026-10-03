import { ChatError } from './request.mjs';

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const maxStreamBytes = 32 * 1024 * 1024;

export function createCompletionAssembler(requestedModel) {
  const toolCalls = new Map();
  let model = requestedModel;
  let role = null;
  let content = '';
  let finishReason;
  let usage = null;

  return {
    push(chunk) {
      if (!isObject(chunk)) return;
      if (typeof chunk.model === 'string' && chunk.model.trim()) model = chunk.model;
      if (chunk.usage !== undefined && chunk.usage !== null) usage = chunk.usage;
      const choice = Array.isArray(chunk.choices) ? chunk.choices[0] : undefined;
      if (!isObject(choice)) return;
      if (typeof choice.finish_reason === 'string') finishReason = choice.finish_reason;
      const delta = choice.delta;
      if (!isObject(delta)) return;
      if (typeof delta.role === 'string' && delta.role) role = delta.role;
      if (typeof delta.content === 'string') content += delta.content;
      if (!Array.isArray(delta.tool_calls)) return;
      for (const call of delta.tool_calls) {
        if (!isObject(call)) continue;
        const index = Number.isSafeInteger(call.index) && call.index >= 0 ? call.index : toolCalls.size;
        const current = toolCalls.get(index) ?? { index, type: 'function', function: { name: '', arguments: '' } };
        if (typeof call.id === 'string' && call.id) current.id = call.id;
        if (typeof call.type === 'string' && call.type) current.type = call.type;
        if (isObject(call.function)) {
          if (typeof call.function.name === 'string') current.function.name += call.function.name;
          if (typeof call.function.arguments === 'string') current.function.arguments += call.function.arguments;
        }
        toolCalls.set(index, current);
      }
    },
    payload() {
      const calls = [...toolCalls.entries()].sort(([left], [right]) => left - right)
        .map(([, { index, ...call }]) => call);
      return {
        model,
        usage,
        choices: [{
          message: { role: role ?? 'assistant', content,
            ...(calls.length ? { tool_calls: calls } : {}) },
          ...(finishReason === undefined ? {} : { finish_reason: finishReason }),
        }],
      };
    },
  };
}

export async function readChatStream(response, requestedModel, { onDelta } = {}) {
  if (onDelta !== undefined && typeof onDelta !== 'function') {
    throw new TypeError('A stream delta observer must be a function.');
  }
  const body = response.body;
  if (!body || typeof body.getReader !== 'function') {
    throw new ChatError('The LLM stream did not contain a readable body.');
  }
  const assembler = createCompletionAssembler(requestedModel);
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  let bytes = 0;
  let done = false;

  const consume = async (block) => {
    for (const line of block.split('\n')) {
      const field = line.startsWith('data:') ? line.slice(5).trim() : '';
      if (!field) continue;
      if (field === '[DONE]') {
        done = true;
        continue;
      }
      let chunk;
      try {
        chunk = JSON.parse(field);
      } catch {
        throw new ChatError('The LLM response was not valid JSON.');
      }
      assembler.push(chunk);
      const text = isObject(chunk) && Array.isArray(chunk.choices)
        ? chunk.choices[0]?.delta?.content : undefined;
      if (onDelta && typeof text === 'string' && text) await onDelta(text);
    }
  };

  try {
    for (;;) {
      let value;
      let finished;
      try {
        ({ value, done: finished } = await reader.read());
      } catch (error) {
        if (error instanceof ChatError) throw error;
        throw new ChatError('The LLM stream ended before the response was complete.', 'response');
      }
      if (finished) break;
      bytes += value?.byteLength ?? 0;
      if (bytes > maxStreamBytes) throw new ChatError('The LLM stream exceeded the supported size.');
      buffer += decoder.decode(value, { stream: true });
      let separator = buffer.search(/\r?\n\r?\n/);
      while (separator !== -1) {
        await consume(buffer.slice(0, separator).replace(/\r/g, ''));
        buffer = buffer.slice(separator).replace(/^\r?\n\r?\n/, '');
        separator = buffer.search(/\r?\n\r?\n/);
      }
      if (done) break;
    }
  } finally {
    await reader.cancel().catch(() => {});
  }
  buffer += decoder.decode();
  if (buffer.trim()) await consume(buffer.replace(/\r/g, ''));
  return assembler.payload();
}
