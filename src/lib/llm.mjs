export async function chatCompletion({ config, messages, tools, env = process.env, fetchImpl = globalThis.fetch }) {
  if (!config.llm.base_url) throw new Error('An LLM base_url is required for chat completion');
  const url = new URL('chat/completions', `${config.llm.base_url.replace(/\/+$/, '')}/`);
  const headers = { 'Content-Type': 'application/json' };
  const key = env[config.llm.api_key_env];
  if (key) headers.Authorization = `Bearer ${key}`;
  let response;
  try {
    response = await fetchImpl(url, {
      method: 'POST',
      headers,
      body: JSON.stringify({ model: config.llm.model, messages, ...(tools ? { tools } : {}) }),
      signal: AbortSignal.timeout(60_000),
    });
  } catch {
    throw new Error('LLM chat request failed (check the endpoint and connection)');
  }
  if (!response.ok) throw new Error(`LLM chat request failed with HTTP ${response.status}`);
  try {
    return await response.json();
  } catch {
    throw new Error('LLM chat endpoint returned invalid JSON');
  }
}
