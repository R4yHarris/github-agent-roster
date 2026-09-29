import assert from 'node:assert/strict';

export function withResearchSummary(fetchImpl) {
  if (!fetchImpl) return undefined;
  return async (url, request) => {
    const body = request?.body ? JSON.parse(request.body) : null;
    if (body?.messages?.[0]?.content?.startsWith('You are the builtin research step.')) {
      assert.equal(body.tools, undefined);
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Inventory reviewed.' } }],
        usage: { prompt_tokens: 0, completion_tokens: 0 },
      }) };
    }
    return await fetchImpl(url, request);
  };
}
