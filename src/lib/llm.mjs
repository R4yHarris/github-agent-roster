import { createChat } from '../llm/openai.mjs';
import { mappedEffort, usesDeepseekReasoning } from '../llm/reasoning.mjs';
import { modelCapabilityPrior } from './capabilities.mjs';

export function createBuiltinChat(config, {
  fetchImpl, env = process.env, vault, onEvent, retryCommand, clock,
} = {}) {
  return createChat({ llm: {
    base_url: config.llm.base_url,
    model: config.llm.model,
    api_key_name: config.llm.api_key_env,
    api_key_optional: config.llm.api_key_optional ?? true,
    request_timeout_ms: config.llm.request_timeout_ms,
    reasoning_effort: mappedEffort(config.llm),
    max_tokens: config.llm.max_tokens,
    ...(usesDeepseekReasoning(config.llm) ? {
      chat_template_kwargs: { thinking: config.llm.effort !== 'none' },
    } : {}),
  } }, { fetch: fetchImpl, env, vault, onEvent: onEvent && ((event) => onEvent({
    ...event, ...(event.type === 'http' ? {
      modelPrior: config.llm.model_prior ?? modelCapabilityPrior(config.llm.model).strength,
    } : {}),
  })), retryCommand, clock });
}

export async function chatCompletion({ config, messages, tools, env, fetchImpl, vault, onEvent, retryCommand }) {
  const chat = createBuiltinChat(config, { fetchImpl, env, vault, onEvent, retryCommand });
  if (chat === null) throw new Error('An LLM base_url is required for chat completion');
  const response = await chat({ messages, ...(tools ? { tools } : {}) });
  return {
    choices: [{ message: response.message, finish_reason: response.finish_reason }],
    usage: response.usage, response: chat.lastResponse,
  };
}
