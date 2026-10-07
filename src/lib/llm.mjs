import { createChat } from '../llm/openai.mjs';
import { docsCompletionCap, mappedEffort, usesDeepseekReasoning } from '../llm/reasoning.mjs';
import { modelCapabilityPrior } from './capabilities.mjs';
import { UnsupportedFinishReasonError } from '../llm/finish-reason.mjs';
import { mergeUsage } from '../metrics/run.mjs';
import * as admission from '../runtime/admission.mjs';

const maxLengthRetries = 3;
const maxReasoningCap = 32_768;
const reasoningCaps = { low: 8192, medium: 16_384, high: 16_384, xhigh: 32_768, max: 32_768 };

// Thinking shares the completion cap with the answer; a fixed 4096 truncates every reasoning turn that writes code.
export function defaultCompletionCap(llm) {
  const effort = mappedEffort(llm);
  const cap = reasoningCaps[effort] ?? 4096;
  const contextBound = Number.isSafeInteger(llm.context_max) && llm.context_max > 0
    ? Math.floor(llm.context_max / 4) : cap;
  return Math.max(4096, Math.min(cap, contextBound));
}

export function createBuiltinChat(config, {
  fetchImpl, env = process.env, vault, onEvent, retryCommand, clock, signal,
  retryLength = true, stream = false,
} = {}) {
  if (typeof retryLength !== 'boolean') throw new TypeError('Length retry permission must be a boolean');
  if (typeof stream !== 'boolean') throw new TypeError('Seat streaming permission must be a boolean');
  const docsSlice = config.llm.task_kind === 'slice' && config.llm.task_class === 'docs';
  const outbound = { maxTokens: null, thinking: null };
  const transport = createChat({ llm: {
    base_url: config.llm.base_url,
    model: config.llm.model,
    api_key_name: config.llm.api_key_env,
    api_key_optional: config.llm.api_key_optional ?? true,
    request_timeout_ms: config.llm.request_timeout_ms,
    stream_idle_timeout_ms: config.llm.stream_idle_timeout_ms,
    // A fleet route measures the profile the router chose; a gateway's response label cannot override it.
    served_model_label: config.llm.locked_model ? 'ignore' : config.llm.served_model_label,
    reasoning_effort: docsSlice ? 'none' : mappedEffort(config.llm),
    max_tokens: config.llm.max_tokens,
    ...(usesDeepseekReasoning(config.llm) ? {
      chat_template_kwargs: { thinking: !docsSlice && config.llm.effort !== 'none' },
    } : {}),
  } }, { fetch: fetchImpl, env, vault,
    onEvent: onEvent && ((event) => onEvent({
    ...event, ...(event.type === 'http' ? {
      modelPrior: config.llm.model_prior ?? modelCapabilityPrior(config.llm.model).strength,
      ...(event.phase === 'start' ? { maxTokens: outbound.maxTokens, thinking: outbound.thinking } : {}),
    } : {}),
  })), retryCommand, clock, signal });
  if (transport === null) return null;
  let completionCap = docsSlice
    ? docsCompletionCap(config.llm) : config.llm.max_tokens ?? defaultCompletionCap(config.llm);
  let reasoningDisabled = docsSlice;
  let lengthRetries = 0;
  let lastAttempts = 0;
  let lastUsage = null;
  const chat = async (request, { signal: requestSignal = signal } = {}) => {
    let current = { ...request, max_tokens: docsSlice
      ? Math.max(request.max_tokens ?? completionCap, completionCap)
      : request.max_tokens ?? completionCap,
      ...(stream ? { stream: request.stream ?? true } : {}),
      ...(reasoningDisabled ? { reasoning_effort: 'none',
        ...(usesDeepseekReasoning(config.llm) ? { chat_template_kwargs: { thinking: false } } : {}),
      } : {}),
    };
    if (!Number.isSafeInteger(current.max_tokens) || current.max_tokens < 1 ||
        current.max_tokens === 1 && lengthRetries === 0) {
      throw new TypeError('Initial builtin completion cap must be an integer of at least 2');
    }
    lastAttempts = 0;
    lastUsage = null;
    let retriedThisRequest = false;
    const usages = [];
    for (;;) {
      lastAttempts += 1;
      outbound.maxTokens = current.max_tokens;
      outbound.thinking = usesDeepseekReasoning(config.llm)
        ? current.chat_template_kwargs?.thinking ?? (!docsSlice && config.llm.effort !== 'none')
        : (current.reasoning_effort ?? (docsSlice ? 'none' : mappedEffort(config.llm))) !== 'none';
      const release = await admit(config, { onEvent, signal: requestSignal });
      try {
        const response = await transport(current, { signal: requestSignal });
        release?.();
        await onEvent?.({ type: 'completion', reason: response.finish_reason ?? null });
        usages.push(response.usage);
        lastUsage = usages.length === 1 ? response.usage : mergeUsage(...usages);
        return { ...response, usage: lastUsage };
      } catch (error) {
        release?.();
        if (!(error instanceof UnsupportedFinishReasonError)) {
          lastUsage = usages.length ? mergeUsage(...usages, null) : null;
          throw error;
        }
        usages.push(transport.lastResponse?.usage ?? null);
        lastUsage = mergeUsage(...usages);
        const retry = retryLength && error.truncated && !retriedThisRequest &&
          lengthRetries < maxLengthRetries && current.max_tokens > 1;
        await onEvent?.({ type: 'finish-reason', reason: error.finishReason, retry,
          ...(retry && docsSlice ? { continued: true } : {}),
        });
        if (!retry) throw error;
        retriedThisRequest = true;
        lengthRetries += 1;
        const resumable = docsSlice && typeof error.partial?.content === 'string' &&
          error.partial.content.trim() && !error.partial.tool_calls?.length
          ? error.partial.content : null;
        // Empty visible output means reasoning spent the cap; only a larger cap can help then.
        const reasoningSpent = !(typeof error.partial?.content === 'string' && error.partial.content.trim()) &&
          !error.partial?.tool_calls?.length;
        // Halving the cap cannot help: the content that overflowed still has to be emitted.
        completionCap = reasoningSpent
          ? Math.max(current.max_tokens, Math.min(current.max_tokens * 2, maxReasoningCap))
          : current.max_tokens;
        current = { ...current, max_tokens: completionCap,
          ...(reasoningDisabled ? { reasoning_effort: 'none',
            ...(usesDeepseekReasoning(config.llm) ? { chat_template_kwargs: { thinking: false } } : {}),
          } : {}),
          messages: [
          ...current.messages,
          ...(resumable === null ? [] : [{ role: 'assistant', content: resumable }]),
          { role: 'user', content:
            (docsSlice ? resumable === null
              ? 'The response stopped at the completion cap. Continue the same answer from where it stopped. '
              : 'That message stopped at the completion cap. Continue it from where it stopped, without repeating it. '
              : reasoningSpent
                ? 'Reasoning used the whole completion cap before any answer. Think briefly, then answer directly. '
                : 'The response was truncated at the completion cap. Split large output: use edit_file for ' +
                  'targeted hunks or several smaller write/edit calls, and keep reasoning brief. ') +
            'Return complete tool calls or a complete summary; do not repeat previously executed edits.' },
        ] };
      }
    }
  };
  Object.defineProperties(chat, {
    lastResponse: { get: () => transport.lastResponse },
    lastAttempts: { get: () => lastAttempts },
    lastUsage: { get: () => lastUsage },
  });
  return chat;
}

// Spec §5 fleet: a fleet profile's concurrency is an in-process admission limit; requests beyond it wait in FIFO order.
const queuedHeartbeatMs = 30_000;

async function admit(config, { onEvent, signal }) {
  const profileId = config.llm?.fleet_profile;
  // Without a fleet profile there is no declared concurrency to enforce; an aborted caller gets the transport's error.
  if (!profileId || signal?.aborted) return undefined;
  if (Number.isSafeInteger(config.llm.concurrency) && config.llm.concurrency > 0) {
    admission.configure(profileId, config.llm.concurrency);
  }
  const pending = admission.acquire(profileId, { signal });
  if (admission.depth(profileId) === 0) return pending;
  const startedAt = Date.now();
  const host = new URL(config.llm.base_url).host;
  const waiting = () => onEvent?.({ type: 'waiting', host, local: false, queued: true,
    elapsedSeconds: Math.floor((Date.now() - startedAt) / 1000), depth: admission.depth(profileId) });
  waiting();
  const heartbeat = setInterval(waiting, queuedHeartbeatMs);
  heartbeat.unref?.();
  try {
    return await pending;
  } finally {
    clearInterval(heartbeat);
  }
}

export async function chatCompletion({ config, messages, tools, env, fetchImpl, vault, onEvent, retryCommand, signal, stream = false }) {
  const chat = createBuiltinChat(config, { fetchImpl, env, vault, onEvent, retryCommand, signal, stream });
  if (chat === null) throw new Error('An LLM base_url is required for chat completion');
  const response = await chat({ messages, ...(tools ? { tools } : {}) });
  return {
    choices: [{ message: response.message, finish_reason: response.finish_reason }],
    usage: response.usage, response: chat.lastResponse,
  };
}
