import assert from 'node:assert/strict';
import { once } from 'node:events';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { gzipSync } from 'node:zlib';
import { formatConfig, parseConfig } from '../src/lib/config.mjs';
import { createBuiltinChat } from '../src/lib/llm.mjs';
import { createRunLog, readLastRunLog, RunLogError } from '../src/lib/run-log.mjs';
import { warmDoctor } from '../src/lib/doctor.mjs';
import { defaultRequestFetch, nodeFetch } from '../src/llm/http.mjs';
import { isLlmTimeout, resolveRequestTimeout } from '../src/llm/request.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://192.168.1.48:8888/v1')
  .replace('model: ""', 'model: spark-model'));
const messages = [{ role: 'user', content: 'PRIVATE_PROMPT' }];
const completion = { choices: [{ message: { role: 'assistant', content: 'PRIVATE_RESPONSE' } }] };

function time(t) {
  t.mock.timers.enable(['setTimeout', 'setInterval']);
  let elapsed = 0;
  return { clock: () => elapsed, tick(ms) { elapsed += ms; t.mock.timers.tick(ms); } };
}

function deferred() {
  let resolve;
  const promise = new Promise((complete) => { resolve = complete; });
  return { promise, resolve };
}

test('local endpoints default exactly20m, cloud120s, and an optional override is validated and preserved', () => {
  for (const host of ['127.0.0.1', '127.0.0.2', 'localhost', '[::1]', '192.168.1.48', '10.0.0.12', '172.30.96.1']) {
    assert.equal(resolveRequestTimeout({ base_url: `http://${host}:8888/v1` }), 1_200_000, host);
  }
  for (const host of ['api.example.test', '192.168.example.test', '10.example.test', '192.169.1.1', '172.32.0.1', '8.8.8.8']) {
    assert.equal(resolveRequestTimeout({ base_url: `https://${host}/v1` }), 120_000, host);
  }
  const override = parseConfig(example.replace('  effort: m', '  request_timeout_ms: 1500000\n  effort: m'));
  assert.equal(override.llm.request_timeout_ms, 1_500_000);
  assert.equal(parseConfig(formatConfig(override)).llm.request_timeout_ms, 1_500_000);
  assert.equal(parseConfig(example).llm.request_timeout_ms, undefined);
  assert.equal(resolveRequestTimeout({ base_url: 'https://api.example.test/v1', request_timeout_ms: 1_500_000 }), 1_500_000);
  for (const value of ['0', '-1', '1.5', '2147483648', '"secret"']) {
    assert.throws(() => parseConfig(example.replace('  effort: m', `  request_timeout_ms: ${value}\n  effort: m`)),
      /request_timeout_ms/);
  }
  assert.equal(defaultRequestFetch(1_200_000), nodeFetch);
  assert.equal(defaultRequestFetch(120_000), globalThis.fetch);
  assert.throws(() => createBuiltinChat(config, { fetchImpl: null }), /fetch implementation/);
});

test('slow Spark chat remains in flight at60s and logs each30s with seat, host and elapsed but no secrets', async (t) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-cold-start-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const timer = time(t);
  let stderr = '';
  const logger = await createRunLog({ repoRoot, session: 'roster-92-coder',
    errorOutput: { write(text) { stderr += String(text); } }, env: { ROSTER_API_KEY: 'PRIVATE_KEY' } });
  const started = deferred();
  const reply = deferred();
  let signal;
  const waiting = [deferred(), deferred(), deferred()];
  let waitingIndex = 0;
  let settled = false;
  const running = logger.seat('planner', 'roster-92-planner', config, async (onEvent) => {
    const chat = createBuiltinChat(config, { env: { ROSTER_API_KEY: 'PRIVATE_KEY' }, clock: timer.clock,
      retryCommand: 'roster run --issue 92',
      onEvent: async (event) => {
        await onEvent(event);
        if (event.type === 'waiting') waiting[waitingIndex++].resolve();
      },
      fetchImpl: async (_url, options) => {
        signal = options.signal;
        started.resolve();
        return reply.promise;
      } });
    return chat({ messages });
  });
  running.then(() => { settled = true; }, () => { settled = true; });
  await started.promise;
  for (const seconds of [30, 60, 90]) {
    timer.tick(30_000);
    await waiting[seconds / 30 - 1].promise;
    assert.ok(readFileSync(logger.path, 'utf8').includes(
      `seat planner waiting host=192.168.1.48:8888 elapsed=${seconds}s cold-start up to 15m`));
    assert.equal((stderr.match(/Still waiting on the model\./g) ?? []).length, seconds / 30 - 1);
    assert.equal(signal.aborted, false);
    assert.equal(settled, false);
  }
  assert.equal((stderr.match(/Still waiting on the model\./g) ?? []).length, 2);
  const tail = await readLastRunLog({ repoRoot, session: 'roster-92-coder', limit: 10, env: {} });
  assert.match(tail.lastLine, /waiting host=192\.168\.1\.48:8888 elapsed=90s cold-start/);
  reply.resolve(Response.json(completion));
  await running;
  const before = stderr;
  timer.tick(120_000);
  await Promise.resolve();
  assert.equal(stderr, before, 'Waiting timer must stop once the response completes');
  assert.doesNotMatch(stderr, /PRIVATE_|\/v1|Authorization|host=|elapsed=|http|chat\.completions|\d{4}-\d\d-\d\dT/);
});

test('local deadline is20m, cloud120s, override wins, and timeout clearly identifies cold-start and retry', async (t) => {
  const timer = time(t);
  for (const [base_url, request_timeout_ms, budget, cold] of [
    ['http://192.168.1.48:8888/v1', undefined, 1_200_000, true],
    ['https://api.example.test/v1', undefined, 120_000, false],
    ['http://10.0.0.5:8000/v1', 90_000, 90_000, true],
  ]) {
    const started = deferred();
    let signal;
    const chat = createBuiltinChat({ ...config, llm: { ...config.llm, base_url, request_timeout_ms } }, {
      env: {}, clock: timer.clock, retryCommand: 'roster run --issue 92 --auto-model',
      fetchImpl: async (_url, options) => { signal = options.signal; started.resolve(); return new Promise(() => {}); },
    });
    const running = chat({ messages });
    const rejected = assert.rejects(running, (error) => {
      assert.equal(error.code, 'ROSTER_LLM_TIMEOUT');
      assert.equal(isLlmTimeout(new Error('wrapper', { cause: error })), true);
      assert.match(error.message, /endpoint timeout, not a bad TASK/);
      assert.match(error.message, /Retry: roster run --issue 92 --auto-model/);
      if (cold) assert.match(error.message, /Cold-start:[\s\S]*host may still be warming/);
      else assert.doesNotMatch(error.message, /Cold-start/);
      assert.doesNotMatch(error.message, /PRIVATE_|\/v1/);
      return true;
    });
    await started.promise;
    timer.tick(budget - 1);
    await Promise.resolve();
    assert.equal(signal.aborted, false);
    timer.tick(1);
    await rejected;
    assert.equal(signal.aborted, true);
  }
});

test('waiting observer failure aborts the request and does not become a successful response or leak timers', async (t) => {
  const timer = time(t);
  const started = deferred();
  let signal;
  const chat = createBuiltinChat(config, { env: {}, clock: timer.clock,
    fetchImpl: async (_url, options) => { signal = options.signal; started.resolve(); return new Promise(() => {}); },
    onEvent: async (event) => { if (event.type === 'waiting') throw new RunLogError('Failed log write'); },
  });
  const running = chat({ messages });
  const rejected = assert.rejects(running, (error) => error instanceof RunLogError);
  await started.promise;
  timer.tick(30_000);
  await rejected;
  assert.equal(signal.aborted, true);
  timer.tick(1_200_000);
});

test('default long transport really tolerates fifteen-minute response headers and compressed JSON without npm deps', async (t) => {
  const received = deferred();
  const server = createServer((request, response) => {
    request.resume();
    received.resolve({ request, response });
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => { server.closeAllConnections(); server.close(); });
  const timer = time(t);
  t.mock.method(globalThis, 'fetch', () => assert.fail('Long requests must not inherit the fetch five-minute limit'));
  const port = server.address().port;
  const chat = createBuiltinChat({ ...config, llm: { ...config.llm, base_url: `http://127.0.0.1:${port}/v1` } }, {
    env: {}, clock: timer.clock,
  });
  let settled = false;
  const running = chat({ messages });
  running.then(() => { settled = true; }, () => { settled = true; });
  const { request, response } = await received.promise;
  timer.tick(900_000);
  await Promise.resolve();
  assert.equal(settled, false);
  assert.equal(request.socket.destroyed, false);
  assert.equal(request.url, '/v1/chat/completions');
  response.writeHead(200, { 'Content-Type': 'application/json', 'Content-Encoding': 'gzip' });
  response.end(gzipSync(JSON.stringify(completion)));
  assert.equal((await running).message.content, 'PRIVATE_RESPONSE');
});

test('optional doctor warm probe uses the same long policy and emits only warming host/status metadata', async (t) => {
  const timer = time(t);
  const started = deferred();
  const reply = deferred();
  let stderr = '';
  let signal;
  const running = warmDoctor({ config, env: { ROSTER_API_KEY: 'PRIVATE_KEY' }, clock: timer.clock,
    errorOutput: { write(text) { stderr += String(text); } },
    fetchImpl: async (url, options) => {
      assert.equal(url, 'http://192.168.1.48:8888/v1/models');
      assert.equal(options.method, 'GET');
      assert.equal(options.redirect, 'error');
      signal = options.signal;
      started.resolve();
      return reply.promise;
    } });
  await started.promise;
  timer.tick(60_000);
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(signal.aborted, false);
  assert.match(stderr, /warming host=192\.168\.1\.48:8888 timeout_ms=1200000/);
  assert.match(stderr, /warming host=192\.168\.1\.48:8888 elapsed=60s/);
  reply.resolve(Response.json({ data: [{ id: 'PRIVATE_MODEL_BODY' }] }));
  await running;
  assert.match(stderr, /warming probe ok host=192\.168\.1\.48:8888 status=200/);
  assert.doesNotMatch(stderr, /PRIVATE_|Authorization|\/v1/);
});

test('doctor warming failure reports cold-start/retry without disclosing request secrets or raw network errors', async () => {
  const timeoutConfig = { ...config, llm: { ...config.llm, request_timeout_ms: 10 } };
  await assert.rejects(warmDoctor({ config: timeoutConfig, env: {}, errorOutput: { write() {} },
    fetchImpl: () => new Promise(() => {}) }), /Cold-start:[\s\S]*warming[\s\S]*Retry: roster doctor --warm/);
  await assert.rejects(warmDoctor({ config, env: {}, errorOutput: { write() {} },
    fetchImpl: async () => { throw new Error('PRIVATE_NETWORK_BODY'); } }),
  (error) => /Warming probe request failed/.test(error.message) && !/PRIVATE_/.test(error.message));
});
