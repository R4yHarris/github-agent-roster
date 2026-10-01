import { createChat } from '../llm/openai.mjs';

export function createBuiltinChat(config, {
  fetchImpl, env = process.env, vault, onEvent, retryCommand, clock,
} = {}) {
  return createChat({ llm: {
    base_url: config.llm.base_url,
    model: config.llm.model,
    api_key_name: config.llm.api_key_env,
    api_key_optional: config.llm.api_key_optional ?? true,
    request_timeout_ms: config.llm.request_timeout_ms,
  } }, { fetch: fetchImpl, env, vault, onEvent, retryCommand, clock });
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
