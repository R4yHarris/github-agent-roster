import { createChat } from '../llm/openai.mjs';

export function createBuiltinChat(config, {
  fetchImpl = globalThis.fetch, env = process.env, vault,
} = {}) {
  return createChat({ llm: {
    base_url: config.llm.base_url,
    model: config.llm.model,
    api_key_name: config.llm.api_key_env,
    api_key_optional: config.llm.api_key_optional ?? true,
    timeout_ms: 60_000,
  } }, { fetch: fetchImpl, env, vault });
}

export async function chatCompletion({ config, messages, tools, env, fetchImpl, vault }) {
  const chat = createBuiltinChat(config, { fetchImpl, env, vault });
  if (chat === null) throw new Error('An LLM base_url is required for chat completion');
  const response = await chat({ messages, ...(tools ? { tools } : {}) });
  return {
    choices: [{ message: response.message, finish_reason: response.finish_reason }],
    usage: response.usage,
  };
}
