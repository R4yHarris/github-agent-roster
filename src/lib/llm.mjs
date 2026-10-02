import { createChat } from '../llm/openai.mjs';
import { mappedEffort, usesDeepseekReasoning } from '../llm/reasoning.mjs';
import { modelCapabilityPrior } from './capabilities.mjs';
import { UnsupportedFinishReasonError } from '../llm/finish-reason.mjs';
import { mergeUsage } from '../metrics/run.mjs';

export function createBuiltinChat(config, {
  fetchImpl, env = process.env, vault, onEvent, retryCommand, clock, signal,
  retryLength = true,
} = {}) {
  if (typeof retryLength !== 'boolean') throw new TypeError('Length retry permission must be a boolean');
  const docsSlice = config.llm.task_kind === 'slice' && config.llm.task_class === 'docs';
  const outbound = { maxTokens: null, thinking: null };
  const transport = createChat({ llm: {
    base_url: config.llm.base_url,
    model: config.llm.model,
    api_key_name: config.llm.api_key_env,
    api_key_optional: config.llm.api_key_optional ?? true,
    request_timeout_ms: config.llm.request_timeout_ms,
    reasoning_effort: docsSlice ? 'none' : mappedEffort(config.llm),
    max_tokens: config.llm.max_tokens,
    ...(usesDeepseekReasoning(config.llm) ? {
      chat_template_kwargs: { thinking: !docsSlice && config.llm.effort !== 'none' },
    } : {}),
  } }, { fetch: fetchImpl, env, vault, onEvent: onEvent && ((event) => onEvent({
    ...event, ...(event.type === 'http' ? {
      modelPrior: config.llm.model_prior ?? modelCapabilityPrior(config.llm.model).strength,
      ...(event.phase === 'start' ? { maxTokens: outbound.maxTokens, thinking: outbound.thinking } : {}),
    } : {}),
  })), retryCommand, clock, signal });
  if (transport === null) return null;
  let completionCap = docsSlice ? Math.min(config.llm.max_tokens ?? 512, 512) : config.llm.max_tokens ?? 4096;
  let reasoningDisabled = docsSlice;
  let lengthRetried = false;
  let lastAttempts = 0;
  let lastUsage = null;
  const chat = async (request, { signal: requestSignal = signal } = {}) => {
    let current = { ...request, max_tokens: request.max_tokens ?? completionCap,
      ...(reasoningDisabled ? { reasoning_effort: 'none',
        ...(usesDeepseekReasoning(config.llm) ? { chat_template_kwargs: { thinking: false } } : {}),
      } : {}),
    };
    if (!Number.isSafeInteger(current.max_tokens) || current.max_tokens < 1 ||
        current.max_tokens === 1 && !lengthRetried) {
      throw new TypeError('Initial builtin completion cap must be an integer of at least 2');
    }
    lastAttempts = 0;
    lastUsage = null;
    const usages = [];
    for (;;) {
      lastAttempts += 1;
      outbound.maxTokens = current.max_tokens;
      outbound.thinking = usesDeepseekReasoning(config.llm)
        ? current.chat_template_kwargs?.thinking ?? (!docsSlice && config.llm.effort !== 'none')
        : (current.reasoning_effort ?? (docsSlice ? 'none' : mappedEffort(config.llm))) !== 'none';
      try {
        const response = await transport(current, { signal: requestSignal });
        await onEvent?.({ type: 'completion', reason: response.finish_reason ?? null });
        usages.push(response.usage);
        lastUsage = usages.length === 1 ? response.usage : mergeUsage(...usages);
        return { ...response, usage: lastUsage };
      } catch (error) {
        if (!(error instanceof UnsupportedFinishReasonError)) {
          lastUsage = usages.length ? mergeUsage(...usages, null) : null;
          throw error;
        }
        usages.push(transport.lastResponse?.usage ?? null);
        lastUsage = mergeUsage(...usages);
        const retry = retryLength && error.truncated && !lengthRetried && current.max_tokens > 1;
        await onEvent?.({ type: 'finish-reason', reason: error.finishReason, retry,
          ...(retry && docsSlice ? { withoutReasoning: true } : {}),
        });
        if (!retry) throw error;
        lengthRetried = true;
        completionCap = docsSlice ? current.max_tokens : Math.floor(current.max_tokens / 2);
        current = { ...current, max_tokens: completionCap,
          ...(reasoningDisabled ? { reasoning_effort: 'none',
            ...(usesDeepseekReasoning(config.llm) ? { chat_template_kwargs: { thinking: false } } : {}),
          } : {}),
          messages: [
          ...current.messages, { role: 'user', content:
            (docsSlice ? 'The response was truncated. Retry without reasoning within the 512 completion cap. '
              : 'The response was truncated. Retry concisely within the smaller completion cap. ') +
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

export async function chatCompletion({ config, messages, tools, env, fetchImpl, vault, onEvent, retryCommand, signal }) {
  const chat = createBuiltinChat(config, { fetchImpl, env, vault, onEvent, retryCommand, signal });
  if (chat === null) throw new Error('An LLM base_url is required for chat completion');
  const response = await chat({ messages, ...(tools ? { tools } : {}) });
  return {
    choices: [{ message: response.message, finish_reason: response.finish_reason }],
    usage: response.usage, response: chat.lastResponse,
  };
}
