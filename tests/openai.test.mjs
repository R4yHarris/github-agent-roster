import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { performance } from 'node:perf_hooks';
import test from 'node:test';
import { inspect } from 'node:util';
import { parseConfig } from '../src/lib/config.mjs';
import { createBuiltinChat } from '../src/lib/llm.mjs';
import { createChat } from '../src/llm/openai.mjs';
import { RunLogError } from '../src/lib/run-log.mjs';

const secret = 'test-only-private-api-key';
const messages = [{ role: 'user', content: 'test-only-private-prompt' }];
const completion = {
  choices: [{ message: { role: 'assistant', content: 'test-only-private-response' } }],
  usage: { prompt_tokens: 4, completion_tokens: 6, total_tokens: 10 },
};

function client(fetch, { llm = {}, env = {}, vault = { get: async () => undefined }, onEvent } = {}) {
  return createChat({
    llm: {
      base_url: 'http://127.0.0.1:8000/v1/',
      model: 'local-test-model',
      api_key_optional: true,
      ...llm,
    },
  }, { fetch, env, vault, onEvent });
}

function safeError(error, pattern) {
  assert.match(error.message, pattern);
  for (const sensitive of [secret, messages[0].content, completion.choices[0].message.content]) {
    assert.ok(!inspect(error).includes(sensitive));
  }
  assert.equal(error.cause, undefined);
  return true;
}

test('empty base_url disables the hook without touching secrets or fetch', () => {
  const unexpected = () => assert.fail('Disabled chat must have no side effects');
  for (const config of [{}, { llm: {} }, { llm: { base_url: '' } }, { llm: { base_url: ' \n' } }]) {
    assert.equal(createChat(config, { fetch: unexpected, vault: { get: unexpected } }), null);
  }
});

test('vLLM chat POSTs to the configured API root without Authorization or logging', async (t) => {
  const logs = [];
  for (const method of ['log', 'info', 'warn', 'error', 'debug']) {
    t.mock.method(console, method, (...args) => logs.push(args));
  }
  const calls = [];
  const chat = client(async (url, options) => {
    calls.push({ url, options });
    return Response.json(completion);
  });
  const result = await chat({ messages, temperature: 0.2, max_tokens: 64 });
  assert.deepEqual(result, { message: completion.choices[0].message, usage: completion.usage,
    model: 'local-test-model' });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, 'http://127.0.0.1:8000/v1/chat/completions');
  assert.equal(calls[0].options.method, 'POST');
  assert.equal(calls[0].options.redirect, 'error');
  assert.deepEqual(calls[0].options.headers, { 'Content-Type': 'application/json' });
  assert.deepEqual(JSON.parse(calls[0].options.body), {
    messages, temperature: 0.2, max_tokens: 64, model: 'local-test-model', stream: false,
  });
  assert.deepEqual(logs, []);
});

test('builtin vllm-local profile preserves usage and its optional-key setting', async () => {
  const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
  const selected = parseConfig(example.replace('profile: ""', 'profile: vllm-local')
    .replace('model: ""', 'model: owner/served-model'));
  const chat = createBuiltinChat(selected, {
    env: {}, vault: { get: async () => undefined },
    fetchImpl: async (url, options) => {
      assert.equal(url, 'http://127.0.0.1:8000/v1/chat/completions');
      assert.equal(options.headers.Authorization, undefined);
      assert.equal(JSON.parse(options.body).model, 'owner/served-model');
      return Response.json(completion);
    },
  });
  assert.deepEqual(await chat({ messages }),
    { message: completion.choices[0].message, usage: completion.usage, model: 'owner/served-model' });

  const keyRequired = parseConfig(example.replace('profile: ""', 'profile: vllm-local')
    .replace('model: ""', 'model: owner/served-model')
    .replace('api_key_optional: true', 'api_key_optional: false'));
  const securedChat = createBuiltinChat(keyRequired, {
    env: {}, vault: { get: async () => undefined },
    fetchImpl: () => assert.fail('Missing a required key must prevent HTTP'),
  });
  await assert.rejects(securedChat({ messages }), /API key is required/);
});
test('hosted chat uses an environment key before the vault and supports model overrides', async () => {
  const chat = client(async (url, options) => {
    assert.equal(url, 'https://api.example.test/v1/chat/completions');
    assert.equal(options.headers.Authorization, `Bearer ${secret}`);
    assert.equal(JSON.parse(options.body).model, 'override-model');
    return Response.json(completion);
  }, {
    llm: { base_url: 'https://api.example.test/v1', api_key_optional: false, api_key_name: 'HOSTED_KEY' },
    env: { HOSTED_KEY: secret },
    vault: { get: () => assert.fail('Environment keys must win') },
  });
  const result = await chat({ messages, model: 'override-model' });
  assert.equal(result.model, 'override-model');
  assert.equal(chat.lastResponse.model, 'override-model');
});

test('a vault key is sent even when keys are optional', async () => {
  const names = [];
  const chat = client(async (url, options) => {
    assert.equal(options.headers.Authorization, `Bearer ${secret}`);
    return Response.json(completion);
  }, {
    vault: { get: async (name) => { names.push(name); return secret; } },
  });
  await chat({ messages });
  assert.deepEqual(names, ['OPENAI_API_KEY']);
});

test('hosted keys are required by default before any HTTP request', async () => {
  const chat = createChat({
    llm: { base_url: 'https://api.example.test/v1', model: 'hosted-model' },
  }, { env: {}, vault: { get: async () => undefined }, fetch: () => assert.fail('Must not fetch') });
  await assert.rejects(chat({ messages }), (error) => safeError(error, /API key is required/));
});

test('tool-call messages survive parsing and omitted usage is null', async () => {
  const message = {
    role: 'assistant',
    content: null,
    tool_calls: [{ id: 'call_1', type: 'function', function: { name: 'lookup', arguments: '{}' } }],
  };
  const chat = client(async () => Response.json({ choices: [{ message }] }));
  assert.deepEqual(await chat({ messages }), { message, usage: null, model: 'local-test-model' });
  assert.deepEqual(chat.lastResponse, { model: 'local-test-model', usage: null });
});

test('passes supported finish reasons through and names rejected reasons without response bodies', async () => {
  const message = { role: 'assistant', content: 'done' };
  const chat = client(async () => Response.json({
    choices: [{ message, finish_reason: 'stop' }],
    usage: { prompt_tokens: 3, completion_tokens: 2 },
  }));
  assert.deepEqual(await chat({ messages }), {
    message, usage: { prompt_tokens: 3, completion_tokens: 2 }, model: 'local-test-model', finish_reason: 'stop',
  });
  const truncated = client(async () => Response.json({
    choices: [{ message, finish_reason: 'length' }],
  }));
  await assert.rejects(truncated({ messages }), /unsupported finish reason: length/);
  assert.deepEqual(truncated.lastResponse, { model: 'local-test-model', usage: null });
});

test('429 retries once with the same payload and key after Retry-After', async () => {
  const calls = [];
  const chat = client(async (url, options) => {
    calls.push({ url, options });
    return calls.length === 1
      ? new Response(secret, { status: 429, headers: { 'Retry-After': '0' } })
      : Response.json(completion);
  }, { env: { OPENAI_API_KEY: secret } });
  assert.deepEqual(await chat({ messages }), { message: completion.choices[0].message, usage: completion.usage,
    model: 'local-test-model' });
  assert.equal(calls.length, 2);
  assert.equal(calls[0].url, calls[1].url);
  assert.equal(calls[0].options.body, calls[1].options.body);
  assert.deepEqual(calls[0].options.headers, calls[1].options.headers);
});

test('a second 429 fails without a third request or leaking its body', async () => {
  let calls = 0;
  const chat = client(async () => {
    calls += 1;
    return new Response(secret, { status: 429, headers: { 'Retry-After': '0' } });
  });
  await assert.rejects(chat({ messages }), (error) => safeError(error, /HTTP 429/));
  assert.equal(calls, 2);
});

test('Retry-After also accepts an HTTP date', async () => {
  let calls = 0;
  const chat = client(async () => {
    calls += 1;
    return calls === 1
      ? new Response('', { status: 429, headers: { 'Retry-After': new Date(Date.now() - 60_000).toUTCString() } })
      : Response.json(completion);
  });
  await chat({ messages });
  assert.equal(calls, 2);
});

test('HTTP errors other than 429 are not retried and their bodies are never parsed', async () => {
  for (const status of [301, 400, 401, 403, 500, 503]) {
    let calls = 0;
    let cancelled = false;
    const chat = client(async () => {
      calls += 1;
      return {
        status,
        body: { cancel: async () => { cancelled = true; } },
        json: () => assert.fail('Error bodies must not be read'),
      };
    });
    await assert.rejects(chat({ messages }), (error) => safeError(error, new RegExp(`HTTP ${status}`)));
    assert.equal(calls, 1);
    assert.equal(cancelled, true);
  }
});

test('network and JSON failures discard sensitive error messages and causes', async () => {
  for (const fetch of [
    async () => { throw new Error(`${secret} ${messages[0].content}`, { cause: new Error(secret) }); },
    async () => new Response(`not JSON: ${secret} ${completion.choices[0].message.content}`),
    async (url, options) => { throw new Error(JSON.stringify(options.headers)); },
  ]) {
    const chat = client(fetch, { env: { OPENAI_API_KEY: secret } });
    await assert.rejects(chat({ messages }), (error) => safeError(error, /request failed|not valid JSON/));
  }
});

test('malformed message and usage shapes fail without returning or exposing bodies', async () => {
  for (const payload of [
    null, {}, { choices: [] }, { choices: [{ message: secret }] },
    { choices: [{ message: { role: 'assistant' } }] },
    { ...completion, usage: secret },
    { ...completion, usage: { prompt_tokens: secret } },
    { ...completion, usage: { total_tokens: -1 } },
  ]) {
    const chat = client(async () => Response.json(payload));
    await assert.rejects(chat({ messages }), (error) => safeError(error, /valid message|invalid usage/));
  }
});

test('the deadline aborts both hanging fetch and hanging response parsing', async () => {
  for (const hangOnBody of [false, true]) {
    let signal;
    const chat = client(async (url, options) => {
      signal = options.signal;
      if (hangOnBody) return { status: 200, json: () => new Promise(() => {}) };
      return new Promise(() => {});
    }, { llm: { timeout_ms: 25 } });
    const started = performance.now();
    await assert.rejects(chat({ messages }), (error) => safeError(error, /timed out/));
    assert.equal(signal.aborted, true);
    assert.ok(performance.now() - started < 2_000, 'Deadline must not wait indefinitely for fetch or JSON');
  }
});

test('retry waits, including the default backoff, stay inside the total deadline', async () => {
  for (const retryAfter of [undefined, 'invalid', '3600', new Date(Date.now() + 60_000).toUTCString()]) {
    let calls = 0;
    const chat = client(async () => {
      calls += 1;
      return new Response(secret, {
        status: 429,
        headers: retryAfter === undefined ? {} : { 'Retry-After': retryAfter },
      });
    }, { llm: { timeout_ms: 25 } });
    const started = performance.now();
    await assert.rejects(chat({ messages }), (error) => safeError(error, /timed out/));
    assert.equal(calls, 1);
    assert.ok(performance.now() - started < 2_000);
  }
});

test('invalid configuration does not echo URL credentials or configuration values', () => {
  for (const llm of [
    { base_url: `https://user:${secret}@api.example.test/v1` },
    { base_url: `https://api.example.test/v1?key=${secret}` },
    { base_url: `https://api.example.test/v1#${secret}` },
    { base_url: `not-a-url-${secret}` },
    { base_url: 'file:///private' },
    { base_url: 123 },
    { api_key_optional: secret },
    { timeout_ms: 0 }, { timeout_ms: 1.5 }, { timeout_ms: 2_147_483_648 },
  ]) {
    assert.throws(() => client(() => assert.fail('Must not fetch'), { llm }),
      (error) => safeError(error, /llm\./));
  }
});

test('invalid and non-serializable requests fail before resolving secrets or making HTTP calls', async () => {
  const chat = client(() => assert.fail('Must not fetch'), {
    vault: { get: () => assert.fail('Must not read secrets') },
  });
  for (const request of [null, {}, { messages: [] }, { messages: [{}] }, { messages, model: '' }, { messages, stream: true }]) {
    await assert.rejects(chat(request), (error) => safeError(error, /request|model|Streaming/));
  }
  await assert.rejects(chat({
    messages,
    extra: { toJSON() { throw new Error(secret); } },
  }), (error) => safeError(error, /JSON serializable/));
});

test('retains only the last successful response model and reported usage without prompts or secret fields', async () => {
  let calls = 0;
  const chat = client(async () => {
    calls += 1;
    if (calls === 3) return new Response(secret, { status: 500 });
    return Response.json(calls === 1 ? {
      ...completion, model: 'actual-served-model',
      usage: { prompt_tokens: 100, completion_tokens: 40, total_tokens: 140, private_field: secret },
    } : { choices: completion.choices });
  });
  assert.equal(chat.lastResponse, null);
  const response = await chat({ messages });
  const snapshot = chat.lastResponse;
  assert.equal(response.model, 'actual-served-model');
  assert.deepEqual(snapshot, { model: 'actual-served-model',
    usage: { prompt_tokens: 100, completion_tokens: 40 } });
  response.usage.prompt_tokens = 999;
  assert.equal(snapshot.usage.prompt_tokens, 100);
  assert.equal(Object.isFrozen(snapshot), true);
  assert.equal(Object.isFrozen(snapshot.usage), true);
  assert.throws(() => { chat.lastResponse = null; }, TypeError);
  for (const sensitive of [secret, messages[0].content, completion.choices[0].message.content]) {
    assert.ok(!JSON.stringify(snapshot).includes(sensitive));
  }

  await chat({ messages, model: 'next-request-model' });
  assert.deepEqual(chat.lastResponse, { model: 'next-request-model', usage: null });
  assert.equal(snapshot.model, 'actual-served-model');
  await assert.rejects(chat({ messages }), (error) => safeError(error, /HTTP 500/));
  assert.deepEqual(chat.lastResponse, { model: 'next-request-model', usage: null });
});

test('live HTTP events expose only model, host, phase, and status without request/response content', async () => {
  const events = [];
  const chat = client(async () => {
    assert.equal(events.at(-1).phase, 'start', 'The start event must be emitted before fetch');
    return Response.json({ ...completion, model: 'actual-model' });
  }, { llm: { base_url: 'http://localhost:8000/private/v1' },
    env: { OPENAI_API_KEY: secret }, onEvent: async (event) => { events.push(event); } });
  await chat({ messages });
  assert.deepEqual(events, [
    { type: 'model', model: 'local-test-model', host: 'localhost:8000' },
    { type: 'http', phase: 'start' },
    { type: 'model', model: 'actual-model', host: 'localhost:8000' },
    { type: 'http', phase: 'ok', status: 200 },
  ]);
  for (const sensitive of [secret, messages[0].content, completion.choices[0].message.content, '/private/v1']) {
    assert.ok(!JSON.stringify(events).includes(sensitive));
  }
});

test('HTTP failures emit safe error classes rather than network or upstream error messages', async () => {
  for (const [fetchImpl, expected] of [
    [async () => new Response('PRIVATE_UPSTREAM_BODY', { status: 503 }), 'http'],
    [async () => { throw new Error('PRIVATE_NETWORK_PROMPT'); }, 'network'],
    [async () => new Response('PRIVATE_INVALID_JSON'), 'response'],
  ]) {
    const events = [];
    const chat = client(fetchImpl, { onEvent: async (event) => { events.push(event); } });
    await assert.rejects(chat({ messages }));
    assert.equal(events.at(-1).phase, 'error');
    assert.equal(events.at(-1).errorClass, expected);
    assert.doesNotMatch(JSON.stringify(events), /PRIVATE_|test-only-private-prompt/);
  }
  const events = [];
  const chat = client(async () => new Promise(() => {}), {
    llm: { timeout_ms: 25 }, onEvent: async (event) => { events.push(event); },
  });
  await assert.rejects(chat({ messages }), /timed out/);
  assert.equal(events.at(-1).errorClass, 'timeout');
});

test('live log failures propagate instead of being mislabeled as HTTP failures', async () => {
  const chat = client(() => assert.fail('A failed logger must stop the request'), {
    onEvent: async () => { throw new RunLogError('Could not persist log'); },
  });
  await assert.rejects(chat({ messages }), (error) => error instanceof RunLogError && /persist log/.test(error.message));
});

test('invalid returned models fail safely rather than falling back to the request model', async () => {
  for (const model of ['', 7, ' padded-model ', 'line\nmodel']) {
    const chat = client(async () => Response.json({ ...completion, model }));
    await assert.rejects(chat({ messages }), (error) => safeError(error, /invalid model/));
    assert.equal(chat.lastResponse, null);
  }
});
