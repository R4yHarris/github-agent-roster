import assert from 'node:assert/strict';
import test from 'node:test';
import { createChat, routeFailure, servedModelMismatch } from '../src/llm/openai.mjs';

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

test('an unrelated response model label is never a route failure', async () => {
  const chat = client('glm-5.3-flash', []);
  const response = await chat(request);
  assert.equal(response.message.content, 'pong');
  assert.equal(routeFailure(Object.assign(new Error('label'), { served: 'glm-5.3-flash' })), null);
});

test('a gateway whose response label is ignored records the requested model and never fails closed', async () => {
  const events = [];
  const chat = createChat({ llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'qwen3.8-27b',
    api_key_optional: true, served_model_label: 'ignore' } }, {
    fetch: async () => Response.json({ model: 'glm-5.3-flash', choices: [{ finish_reason: 'stop',
      message: { role: 'assistant', content: 'pong' } }] }),
    env: {}, vault: { get: async () => undefined },
    onEvent: (event) => { events.push(event); },
  });
  const response = await chat(request);
  assert.equal(response.model, 'qwen3.8-27b');
  assert.equal(chat.lastResponse.model, 'qwen3.8-27b');
  assert.equal(events.some(({ type }) => type === 'served-model'), false);
  assert.throws(() => createChat({ llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'm',
    served_model_label: 'maybe' } }), /served_model_label/);
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

test('a transient gateway error body or 5xx is retried once, and a repeat is a route failure', async () => {
  const replies = [
    () => new Response('{"error":{"message":"backend down"}}', { headers: sse }),
    () => new Response('data: {"model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"pong"},' +
      '"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n', { headers: sse }),
  ];
  let calls = 0;
  const chat = createChat({ llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'm', api_key_optional: true } }, {
    fetch: async () => replies[calls++](), env: {}, vault: { get: async () => undefined },
  });
  assert.equal((await chat(streamRequest)).message.content, 'pong');
  assert.equal(calls, 2);

  let failing = 0;
  const down = createChat({ llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'm', api_key_optional: true } }, {
    fetch: async () => { failing += 1; return new Response('bad gateway', { status: 502 }); },
    env: {}, vault: { get: async () => undefined },
  });
  await assert.rejects(down(streamRequest), (error) => {
    assert.match(error.message, /HTTP 502/);
    assert.deepEqual(routeFailure(error), { reason: 'endpoint-error' });
    return true;
  });
  assert.equal(failing, 2);

  let unauthorized = 0;
  const denied = createChat({ llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'm', api_key_optional: true } }, {
    fetch: async () => { unauthorized += 1; return new Response('no', { status: 401 }); },
    env: {}, vault: { get: async () => undefined },
  });
  await assert.rejects(denied(streamRequest), (error) => routeFailure(error) === null);
  assert.equal(unauthorized, 1);
});

test('a stream with no data frames is an error, not an empty completion', async () => {
  const chat = createChat({ llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'm', api_key_optional: true } }, {
    fetch: async () => new Response(': keep-alive\n\n', { headers: sse }),
    env: {}, vault: { get: async () => undefined },
  });
  await assert.rejects(chat(streamRequest), /no completion data/);
});
