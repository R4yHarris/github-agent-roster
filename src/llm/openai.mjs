import { setTimeout as delay } from 'node:timers/promises';
import { resolveSecret } from '../lib/secrets.mjs';
import { redactSecrets } from '../runtime/memory.mjs';
import { defaultRequestFetch } from './http.mjs';
import { UnsupportedFinishReasonError } from './finish-reason.mjs';
import { ChatError, isLocalLlmHost, LlmTimeoutError, resolveRequestTimeout, validateRetryCommand,
  withRequestTimeout } from './request.mjs';
const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

function endpointFor(baseUrl) {
  let url;
  try {
    url = new URL(baseUrl);
  } catch {
    throw new TypeError('llm.base_url must be an HTTP or HTTPS URL.');
  }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) {
    throw new TypeError('llm.base_url must be an HTTP or HTTPS URL without credentials, a query or a fragment.');
  }
  url.pathname = `${url.pathname.replace(/\/+$/, '')}/chat/completions`;
  return url.href;
}

function retryDelay(response, timeoutMs) {
  const value = response.headers.get('retry-after');
  if (value === null) return Math.min(1_000, timeoutMs);
  const milliseconds = /^\d+(?:\.\d+)?$/.test(value)
    ? Number(value) * 1_000
    : Date.parse(value) - Date.now();
  return Number.isFinite(milliseconds)
    ? Math.min(Math.max(0, milliseconds), timeoutMs)
    : Math.min(1_000, timeoutMs);
}

function parseCompletion(payload, requestedModel) {
  const choice = payload?.choices?.[0];
  const message = choice?.message;
  if (!isObject(message) || typeof message.role !== 'string' || !message.role ||
      !(typeof message.content === 'string' || message.content === null ||
        Array.isArray(message.content) || Array.isArray(message.tool_calls) || isObject(message.function_call))) {
    throw new ChatError('The LLM response did not contain a valid message.');
  }
  if (choice.finish_reason !== undefined && choice.finish_reason !== null &&
      typeof choice.finish_reason !== 'string') {
    throw new ChatError('The LLM response had an invalid finish reason.');
  }
  const usage = payload.usage ?? null;
  if (usage !== null && (!isObject(usage) ||
      ['prompt_tokens', 'completion_tokens', 'total_tokens'].some((field) =>
        usage[field] !== undefined && (!Number.isSafeInteger(usage[field]) || usage[field] < 0)))) {
    throw new ChatError('The LLM response contained invalid usage.');
  }
  const model = payload.model ?? requestedModel;
  if (typeof model !== 'string' || !model.trim() || model !== model.trim() || /[\r\n\0]/.test(model)) {
    throw new ChatError('The LLM response contained an invalid model.');
  }
  return {
    message: Object.fromEntries(Object.entries(message).filter(([key]) => key !== 'reasoning_content')), usage, model,
    ...(choice.finish_reason === undefined ? {} : { finish_reason: choice.finish_reason }),
  };
}

export function createChat(config = {}, {
  fetch: suppliedFetch, env = process.env, vault, onEvent, retryCommand, clock,
} = {}) {
  if (!isObject(config) || (config.llm !== undefined && !isObject(config.llm))) {
    throw new TypeError('LLM configuration must be an object with an optional llm object.');
  }
  const llm = config.llm ?? {};
  if (llm.base_url === undefined || (typeof llm.base_url === 'string' && !llm.base_url.trim())) {
    return null;
  }
  if (typeof llm.base_url !== 'string') throw new TypeError('llm.base_url must be a string.');
  const endpoint = endpointFor(llm.base_url);
  const optionalKey = llm.api_key_optional ?? false;
  const timeoutMs = resolveRequestTimeout(llm);
  const fetchImpl = suppliedFetch === undefined ? defaultRequestFetch(timeoutMs) : suppliedFetch;
  if (typeof optionalKey !== 'boolean') throw new TypeError('llm.api_key_optional must be a boolean.');
  if (typeof fetchImpl !== 'function') throw new TypeError('A fetch implementation is required.');
  if (onEvent !== undefined && typeof onEvent !== 'function') throw new TypeError('Live chat observer must be a function.');
  validateRetryCommand(retryCommand);
  const url = new URL(endpoint);
  const host = redactSecrets(url.host, { env, apiKeyEnv: llm.api_key_name ?? 'OPENAI_API_KEY' });
  const local = isLocalLlmHost(url.hostname);

  let lastResponse = null;
  const chat = async function chat(request) {
    if (!isObject(request) || !Array.isArray(request.messages) || request.messages.length === 0 ||
        request.messages.some((message) => !isObject(message) || typeof message.role !== 'string' || !message.role)) {
      throw new TypeError('A chat request requires a non-empty messages array with message roles.');
    }
    const model = request.model ?? llm.model;
    if (typeof model !== 'string' || !model.trim()) throw new TypeError('A chat model is required.');
    if (request.stream !== undefined && request.stream !== false) {
      throw new TypeError('Streaming chat responses are not supported.');
    }
    let body;
    try {
      body = JSON.stringify({
        ...(llm.reasoning_effort === undefined ? {} : { reasoning_effort: llm.reasoning_effort }),
        ...(llm.max_tokens === undefined ? {} : { max_tokens: llm.max_tokens }),
        ...(llm.chat_template_kwargs === undefined ? {} : { chat_template_kwargs: llm.chat_template_kwargs }),
        ...request, model, stream: false,
      });
    } catch {
      throw new TypeError('The chat request must be JSON serializable.');
    }
    const key = await resolveSecret(llm.api_key_name ?? 'OPENAI_API_KEY', { env, vault });
    if (!key && !optionalKey) {
      await onEvent?.({ type: 'http', phase: 'error', errorClass: 'authentication' });
      throw new ChatError('An LLM API key is required. Set the configured environment variable or vault entry.', 'authentication');
    }
    const headers = { 'Content-Type': 'application/json' };
    if (key) headers.Authorization = `Bearer ${key}`;
    let status;
    async function send(signal) {
      for (let attempt = 0; attempt < 2; attempt += 1) {
        signal.throwIfAborted();
        await onEvent?.({ type: 'model', model: key ? model.split(key).join('[redacted]') : model, host });
        await onEvent?.({ type: 'http', phase: 'start',
          ...(llm.reasoning_effort === undefined ? {} : { effort: llm.reasoning_effort }) });
        signal.throwIfAborted();
        const response = await fetchImpl(endpoint, {
          method: 'POST',
          headers,
          body,
          signal,
          redirect: 'error',
        });
        signal.throwIfAborted();
        if (!Number.isInteger(response.status) || response.status < 100 || response.status > 599) {
          throw new ChatError('The LLM response had an invalid HTTP status.');
        }
        status = response.status;
        if (response.status === 429 && attempt === 0) {
          await onEvent?.({ type: 'http', phase: 'error', status, errorClass: 'http' });
          await response.body?.cancel();
          await delay(retryDelay(response, timeoutMs), undefined, { signal });
          continue;
        }
        if (response.status < 200 || response.status >= 300) {
          await response.body?.cancel();
          throw new ChatError(`The LLM request failed (HTTP ${response.status}).`, 'http');
        }
        let payload;
        try {
          payload = await response.json();
        } catch {
          throw new ChatError('The LLM response was not valid JSON.');
        }
        signal.throwIfAborted();
        const completion = parseCompletion(payload, model);
        await onEvent?.({ type: 'model',
          model: key ? completion.model.split(key).join('[redacted]') : completion.model, host });
        await onEvent?.({ type: 'http', phase: 'ok', status });
        return completion;
      }
    }

    try {
      const response = await withRequestTimeout(send, {
        host, local, timeoutMs, retryCommand, clock,
        ...(onEvent ? { onWaiting: (event) => onEvent({ type: 'waiting', ...event }) } : {}),
      });
      const usage = response.usage === null ? null : Object.freeze(Object.fromEntries(
        ['prompt_tokens', 'completion_tokens'].filter((field) => response.usage[field] !== undefined)
          .map((field) => [field, response.usage[field]]),
      ));
      lastResponse = Object.freeze({ model: response.model, usage });
      if (response.finish_reason != null && !['stop', 'tool_calls'].includes(response.finish_reason)) {
        const rawReason = key ? response.finish_reason.split(key).join('[redacted]') : response.finish_reason;
        const reason = redactSecrets(rawReason, {
          env, apiKeyEnv: llm.api_key_name ?? 'OPENAI_API_KEY',
        }).replace(/[\x00-\x1f\x7f]/g, '?').slice(0, 128) || '(empty)';
        throw new UnsupportedFinishReasonError(reason, { truncated: response.finish_reason === 'length' });
      }
      return response;
    } catch (error) {
      if (error?.code === 'ROSTER_RUN_LOG') throw error;
      if (error instanceof LlmTimeoutError) {
        await onEvent?.({ type: 'timeout', host, local, timeoutMs, retryCommand });
      }
      await onEvent?.({ type: 'http', phase: 'error', ...(status === undefined ? {} : { status }),
        errorClass: error instanceof ChatError ? error.category : error?.name === 'AbortError' ? 'abort' : 'network' });
      if (error instanceof ChatError) throw error;
      throw new ChatError('The LLM request failed. Check the endpoint and connection.', 'network');
    }
  };
  Object.defineProperty(chat, 'lastResponse', { get: () => lastResponse });
  return chat;
}
