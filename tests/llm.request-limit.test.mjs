import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { createChat, ModelRequestLimitError, routeFailure } from '../src/llm/openai.mjs';
import { createBuiltinChat } from '../src/lib/llm.mjs';
import { parseConfig } from '../src/lib/config.mjs';

const request = { messages: [{ role: 'user', content: 'fixture' }] };
const llm = { base_url: 'http://localhost:3456/v1', model: 'fixture', api_key_optional: true };
const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const response = (reason = 'stop') => Response.json({
  choices: [{ finish_reason: reason, message: { role: 'assistant', content: 'ok' } }],
});

test('exact ceiling counts actual sends and fails without endpoint quarantine or usage fabrication', async () => {
  let sends = 0;
  const chat = createChat({ llm: { ...llm, max_requests: 2 } }, { env: {},
    fetch: async () => { sends += 1; return response(); } });
  assert.equal(chat.requestCount, 0);
  for (let index = 0; index < 2; index += 1) assert.equal((await chat(request)).usage, null);
  await assert.rejects(chat(request), (error) => {
    assert.ok(error instanceof ModelRequestLimitError);
    assert.equal(error.code, 'ROSTER_MODEL_REQUEST_LIMIT');
    assert.equal(routeFailure(error), null);
    return true;
  });
  assert.equal(sends, 2);
  assert.equal(chat.requestCount, 2);
  assert.equal(chat.requestLimit, 2);
  assert.throws(() => { chat.requestCount = 0; }, TypeError);
});

test('concurrent callers cannot overrun their shared HTTP ceiling', async () => {
  let sends = 0;
  const chat = createChat({ llm: { ...llm, max_requests: 2 } }, { env: {},
    fetch: async () => { sends += 1; await new Promise((resolve) => setTimeout(resolve, 5)); return response(); } });
  const results = await Promise.allSettled(Array.from({ length: 5 }, () => chat(request)));
  assert.equal(results.filter(({ status }) => status === 'fulfilled').length, 2);
  assert.equal(sends, 2);
  assert.equal(chat.requestCount, 2);
  for (const result of results.filter(({ status }) => status === 'rejected')) {
    assert.ok(result.reason instanceof ModelRequestLimitError);
  }
});

test('HTTP retry cannot send beyond the budget', async () => {
  let sends = 0;
  const chat = createChat({ llm: { ...llm, max_requests: 1 } }, { env: {}, fetch: async () => {
    sends += 1;
    return new Response('', { status: 503, headers: { 'retry-after': '0' } });
  } });
  await assert.rejects(chat(request), ModelRequestLimitError);
  assert.equal(sends, 1);
});

test('failed HTTP attempts consume the ceiling without changing usage', async () => {
  let sends = 0;
  const chat = createChat({ llm: { ...llm, max_requests: 1 } }, { env: {}, fetch: async () => {
    sends += 1;
    throw new Error('fixture connection failure');
  } });
  await assert.rejects(chat(request), /LLM request failed/);
  await assert.rejects(chat(request), ModelRequestLimitError);
  assert.equal(sends, 1);
  assert.equal(chat.lastResponse, null);
});

test('transport ceilings are fixed at creation and fresh contexts start at zero', async () => {
  const settings = { ...llm, max_requests: 1 };
  const options = { env: {}, fetch: async () => response() };
  const chat = createChat({ llm: settings }, options);
  settings.max_requests = 3;
  await chat(request);
  await assert.rejects(chat(request), ModelRequestLimitError);
  assert.equal(chat.requestLimit, 1);
  const fresh = createChat({ llm: settings }, options);
  assert.equal(fresh.requestCount, 0);
  assert.equal(fresh.requestLimit, 3);
  await fresh(request);
  assert.equal(fresh.requestCount, 1);
});

test('builtin completion-length retry consumes the same actual-send ceiling', async () => {
  const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
    .replace('model: ""', 'model: fixture'));
  let sends = 0;
  const chat = createBuiltinChat({ ...config, llm: { ...config.llm, max_requests: 1 } }, {
    env: {}, fetchImpl: async () => { sends += 1; return response('length'); },
  });
  await assert.rejects(chat(request), ModelRequestLimitError);
  assert.equal(sends, 1);
  assert.equal(chat.requestCount, 1);
  assert.equal(chat.requestLimit, 1);
  assert.equal(chat.lastUsage?.prompt_tokens, undefined);
  assert.equal(chat.lastUsage?.completion_tokens, undefined);
});

test('validation and cancellation before HTTP do not consume requests', async () => {
  const chat = createChat({ llm: { ...llm, max_requests: 1 } }, { env: {}, fetch: async () => response() });
  await assert.rejects(chat({ messages: [] }), TypeError);
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(chat(request, { signal: controller.signal }));
  assert.equal(chat.requestCount, 0);
  await chat(request);
  assert.equal(chat.requestCount, 1);
});

test('unset limit preserves repeated calls and configuration rejects invalid caps', async () => {
  const chat = createChat({ llm }, { env: {}, fetch: async () => response() });
  await Promise.all(Array.from({ length: 4 }, () => chat(request)));
  assert.equal(chat.requestCount, 4);
  assert.equal(chat.requestLimit, null);
  assert.equal(parseConfig(example).llm.max_requests, undefined);
  assert.equal(parseConfig(example.replace('llm:\n', 'llm:\n  max_requests: 2\n')).llm.max_requests, 2);
  for (const value of [0, -1, 1.5, 10001, '2', null]) {
    assert.throws(() => createChat({ llm: { ...llm, max_requests: value } }), /max_requests/);
  }
  assert.throws(() => parseConfig(example.replace('llm:\n', 'llm:\n  max_requests: 0\n')), /max_requests/);
});
