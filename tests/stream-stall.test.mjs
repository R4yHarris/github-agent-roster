import assert from 'node:assert/strict';
import test from 'node:test';
import { createChat } from '../src/llm/openai.mjs';
import { isLlmTimeout, remoteStreamIdleTimeoutMs, resolveStreamIdleTimeout } from '../src/llm/request.mjs';

const headers = { 'content-type': 'text/event-stream' };
const encoder = new TextEncoder();
const frame = (chunk) => `data: ${JSON.stringify(chunk)}\n\n`;
const role = frame({ model: 'm', choices: [{ delta: { role: 'assistant' } }] });
const text = (content) => frame({ model: 'm', choices: [{ delta: { content } }] });
const done = frame({ model: 'm', choices: [{ delta: {}, finish_reason: 'stop' }],
  usage: { prompt_tokens: 3, completion_tokens: 2 } }) + 'data: [DONE]\n\n';

function hanging(signal) {
  return new Response(new ReadableStream({
    start(controller) {
      controller.enqueue(encoder.encode(role));
      signal.addEventListener('abort', () => controller.error(new DOMException('aborted', 'AbortError')),
        { once: true });
    },
  }), { headers });
}

function complete() {
  return new Response(role + text('pong') + done, { headers });
}

function slow(parts, gapMs) {
  return new Response(new ReadableStream({
    async start(controller) {
      for (const part of parts) {
        controller.enqueue(encoder.encode(part));
        await new Promise((resolve) => setTimeout(resolve, gapMs));
      }
      controller.close();
    },
  }), { headers });
}

function client(fetch, events, idle = 50) {
  return createChat({ llm: { base_url: 'http://127.0.0.1:8000/v1', model: 'm', api_key_optional: true,
    stream_idle_timeout_ms: idle } }, {
    fetch, env: {}, vault: { get: async () => undefined },
    onEvent: (event) => { events.push(event); },
  });
}

const request = { stream: true, messages: [{ role: 'user', content: 'ping' }] };

test('a silent stream is abandoned and the same request retried once', async () => {
  const events = [];
  const bodies = [];
  const chat = client(async (_url, init) => {
    bodies.push(init.body);
    return bodies.length === 1 ? hanging(init.signal) : complete();
  }, events);
  const response = await chat(request);
  assert.equal(response.message.content, 'pong');
  assert.equal(bodies.length, 2);
  assert.equal(bodies[0], bodies[1]);
  assert.deepEqual(events.filter((event) => event.type === 'stall'),
    [{ type: 'stall', host: '127.0.0.1:8000', idleSeconds: 0.05, retry: true }]);
});

test('a second stall fails as an endpoint stall without a third request', async () => {
  const events = [];
  let calls = 0;
  const chat = client(async (_url, init) => { calls += 1; return hanging(init.signal); }, events);
  await assert.rejects(chat(request), (error) => {
    assert.equal(error.code, 'ROSTER_LLM_STALL');
    assert.equal(error.category, 'timeout');
    assert.ok(isLlmTimeout(error));
    assert.match(error.message, /not a bad TASK/);
    return true;
  });
  assert.equal(calls, 2);
  assert.deepEqual(events.filter((event) => event.type === 'stall').map((event) => event.retry), [true, false]);
  assert.deepEqual(events.filter((event) => event.type === 'http' && event.phase === 'error')
    .map((event) => event.errorClass), ['timeout']);
});

test('a slow stream that keeps sending bytes is never treated as stalled', async () => {
  const events = [];
  let calls = 0;
  const chat = client(async () => { calls += 1; return slow([role, text('p'), text('o'), text('ng'), done], 30); },
    events, 80);
  const response = await chat(request);
  assert.equal(response.message.content, 'pong');
  assert.equal(calls, 1);
  assert.equal(events.some((event) => event.type === 'stall'), false);
});

test('waiting for response headers is a cold start, not a stall', async () => {
  const events = [];
  let calls = 0;
  const chat = client(async () => {
    calls += 1;
    await new Promise((resolve) => setTimeout(resolve, 150));
    return complete();
  }, events, 40);
  const response = await chat(request);
  assert.equal(response.message.content, 'pong');
  assert.equal(calls, 1);
  assert.equal(events.some((event) => event.type === 'stall'), false);
});

test('non-streaming requests are not watched', async () => {
  const events = [];
  const chat = client(async () => {
    await new Promise((resolve) => setTimeout(resolve, 120));
    return Response.json({ model: 'm', choices: [{ message: { role: 'assistant', content: 'pong' } }] });
  }, events, 30);
  const response = await chat({ messages: request.messages });
  assert.equal(response.message.content, 'pong');
  assert.equal(events.some((event) => event.type === 'stall'), false);
});

test('only remote hosts get a default idle watchdog', () => {
  assert.equal(resolveStreamIdleTimeout({ base_url: 'http://127.0.0.1:11435/v1' }), 0);
  assert.equal(resolveStreamIdleTimeout({ base_url: 'http://192.168.1.48:8888/v1' }), 0);
  assert.equal(resolveStreamIdleTimeout({ base_url: 'https://aperture.example.ts.net/v1' }),
    remoteStreamIdleTimeoutMs);
  assert.equal(resolveStreamIdleTimeout({ base_url: 'https://aperture.example.ts.net/v1',
    stream_idle_timeout_ms: 0 }), 0);
  assert.throws(() => resolveStreamIdleTimeout({ base_url: 'http://127.0.0.1/v1', stream_idle_timeout_ms: -1 }));
});
