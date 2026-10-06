import assert from 'node:assert/strict';
import test from 'node:test';
import { createChat, servedModelMismatch } from '../src/llm/openai.mjs';

function client(served, events) {
  return createChat({ llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'qwen3.8-27b', api_key_optional: true } }, {
    fetch: async () => Response.json({ model: served, choices: [{ finish_reason: 'stop',
      message: { role: 'assistant', content: 'pong' } }] }),
    env: {}, vault: { get: async () => undefined },
    onEvent: (event) => { events.push(event); },
  });
}

const request = { messages: [{ role: 'user', content: 'ping' }] };

test('a gateway alias to an unrelated model is reported once per client', async () => {
  const events = [];
  const chat = client('glm-5.3-flash', events);
  await chat(request);
  await chat(request);
  assert.deepEqual(events.filter(({ type }) => type === 'served-model'), [
    { type: 'served-model', host: '127.0.0.1:8000', requested: 'qwen3.8-27b', served: 'glm-5.3-flash' },
  ]);
  assert.equal(chat.lastResponse.model, 'glm-5.3-flash');
});

test('a locked fleet model fails closed when the gateway serves an unrelated model', async () => {
  const events = [];
  const chat = createChat({
    llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'qwen3.8-27b', api_key_optional: true },
  }, {
    fetch: async () => Response.json({ model: 'glm-5.3-flash', choices: [{ finish_reason: 'stop',
      message: { role: 'assistant', content: 'pong' } }] }),
    env: {}, vault: { get: async () => undefined }, expectedModel: 'qwen3.8-27b',
    onEvent: (event) => { events.push(event); },
  });
  await assert.rejects(chat(request), /Locked fleet model mismatch: requested qwen3\.8-27b, served glm-5\.3-flash/);
  assert.equal(chat.lastResponse.model, 'glm-5.3-flash');
  assert.equal(events.filter(({ type }) => type === 'served-model').length, 1);
});

test('the same model with a path prefix or tag is not a mismatch', async () => {
  const events = [];
  await client('Qwen/qwen3.8-27b', events)(request);
  assert.equal(events.some(({ type }) => type === 'served-model'), false);
  assert.equal(servedModelMismatch('qwen3.8:27b', 'qwen3.8:27b-smtek'), false);
  assert.equal(servedModelMismatch('deepseek-v4-flash', 'glm-5.3-flash'), true);
});

test('unsafe model names are never echoed as a mismatch', () => {
  assert.equal(servedModelMismatch('qwen', 'glm "injected"'), false);
  assert.equal(servedModelMismatch('qwen', 'x'.repeat(129)), false);
});

test('a locked fleet model ID must use the safe served-model alphabet', () => {
  assert.throws(() => createChat({
    llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'qwen3.8-27b', api_key_optional: true },
  }, { expectedModel: 'unsafe model', fetch: async () => assert.fail('must fail before fetch') }),
  /Expected model must be a supported served model ID/);
});

const sse = { 'content-type': 'text/event-stream' };
const streamRequest = { stream: true, tools: [], messages: [{ role: 'user', content: 'ping' }] };

test('an HTTP 200 JSON error body on a stream is an error, and empty tools are not sent', async () => {
  let body;
  const chat = createChat({ llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'm', api_key_optional: true } }, {
    fetch: async (_url, init) => {
      body = JSON.parse(init.body);
      return new Response('{"error":{"message":"all backends failed for role planning @ https://10.0.0.9"}}', { headers: sse });
    },
    env: {}, vault: { get: async () => undefined },
  });
  await assert.rejects(chat(streamRequest), (error) => {
    assert.match(error.message, /error body instead of a stream/);
    assert.doesNotMatch(error.message, /10\.0\.0\.9/);
    return true;
  });
  assert.equal(Object.hasOwn(body, 'tools'), false);
});

test('a stream with no data frames is an error, not an empty completion', async () => {
  const chat = createChat({ llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'm', api_key_optional: true } }, {
    fetch: async () => new Response(': keep-alive\n\n', { headers: sse }),
    env: {}, vault: { get: async () => undefined },
  });
  await assert.rejects(chat(streamRequest), /no completion data/);
});
