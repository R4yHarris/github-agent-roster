import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { withFleetProfile } from '../src/lib/fleet.mjs';
import { chatCompletion, createBuiltinChat } from '../src/lib/llm.mjs';
import { depth } from '../src/runtime/admission.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const base = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model'));
const profile = (id, concurrency) => ({ id, base_url: 'http://gpu-a:8000/v1', model: 'local-model', provider: 'vllm',
  context_max: 32768, concurrency, hardware: 'test gpu', notes: '' });
const reply = () => Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'ok' } }],
  usage: { prompt_tokens: 1, completion_tokens: 1 } });

// A fake endpoint that holds every request open until the test releases it.
function gatedEndpoint() {
  const gates = [];
  let inFlight = 0;
  let maxInFlight = 0;
  const order = [];
  const fetchImpl = async (_url, request) => {
    inFlight += 1;
    maxInFlight = Math.max(maxInFlight, inFlight);
    order.push(JSON.parse(request.body).messages.at(-1).content);
    await new Promise((resolve) => gates.push(resolve));
    inFlight -= 1;
    return reply();
  };
  const until = async (ready) => {
    for (const start = Date.now(); !ready();) {
      assert.ok(Date.now() - start < 5000, 'endpoint state not reached');
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
  };
  // Gives a request that must stay queued time to wrongly reach the endpoint.
  const settle = () => new Promise((resolve) => setTimeout(resolve, 50));
  return { fetchImpl, gates, order, until, settle, stats: () => ({ inFlight, maxInFlight }) };
}

const ask = (config, fetchImpl, content, extra = {}) => chatCompletion({ config, fetchImpl, env: {},
  messages: [{ role: 'user', content }], ...extra });

test('withFleetProfile carries the fleet profile id and concurrency into the seat config (#343)', () => {
  const config = withFleetProfile(base, profile('gpu-a', 3));
  assert.equal(config.llm.fleet_profile, 'gpu-a');
  assert.equal(config.llm.concurrency, 3);
});

test('requests beyond a fleet profile concurrency wait FIFO and report their queued depth (#343)', async () => {
  const config = withFleetProfile(base, profile('admission-fifo', 2));
  const endpoint = gatedEndpoint();
  const events = [];
  const calls = ['a', 'b', 'c', 'd'].map((content) =>
    ask(config, endpoint.fetchImpl, content, { onEvent: (event) => events.push(event) }));
  await endpoint.until(() => endpoint.gates.length === 2);
  await endpoint.settle();
  assert.deepEqual(endpoint.order, ['a', 'b']);
  assert.equal(depth('admission-fifo'), 2);
  assert.deepEqual(events.filter((event) => event.queued).map(({ depth: queued }) => queued), [1, 2]);
  for (let opened = 0; opened < 4; opened += 1) {
    await endpoint.until(() => endpoint.gates.length > 0);
    endpoint.gates.shift()();
  }
  await Promise.all(calls);
  assert.deepEqual(endpoint.order, ['a', 'b', 'c', 'd']);
  assert.equal(endpoint.stats().maxInFlight, 2);
  assert.equal(depth('admission-fifo'), 0);
});

test('an aborted queued request leaks no slot and the next waiter proceeds (#343)', async () => {
  const config = withFleetProfile(base, profile('admission-abort', 1));
  const endpoint = gatedEndpoint();
  const first = ask(config, endpoint.fetchImpl, 'first');
  const controller = new AbortController();
  const aborted = ask(config, endpoint.fetchImpl, 'aborted', { signal: controller.signal });
  const next = ask(config, endpoint.fetchImpl, 'next');
  await endpoint.until(() => endpoint.gates.length === 1 && depth('admission-abort') === 2);
  controller.abort();
  await assert.rejects(aborted, { name: 'AbortError' });
  endpoint.gates.shift()();
  await first;
  await endpoint.until(() => endpoint.gates.length === 1);
  endpoint.gates.shift()();
  await next;
  assert.deepEqual(endpoint.order, ['first', 'next']);
  assert.equal(depth('admission-abort'), 0);
  const after = ask(config, endpoint.fetchImpl, 'after');
  await endpoint.until(() => endpoint.gates.length === 1);
  endpoint.gates.shift()();
  await after;
});

test('seat chats built directly share the fleet profile admission limit (#343)', async () => {
  const config = withFleetProfile(base, profile('admission-seat', 1));
  const endpoint = gatedEndpoint();
  const chats = ['coder', 'reviewer'].map(() => createBuiltinChat(config, { fetchImpl: endpoint.fetchImpl, env: {} }));
  const calls = chats.map((chat, index) => chat({ messages: [{ role: 'user', content: String(index) }] }));
  await endpoint.until(() => endpoint.gates.length === 1);
  await endpoint.settle();
  assert.deepEqual(endpoint.order, ['0']);
  endpoint.gates.shift()();
  await endpoint.until(() => endpoint.gates.length === 1);
  endpoint.gates.shift()();
  await Promise.all(calls);
  assert.equal(endpoint.stats().maxInFlight, 1);
  assert.equal(depth('admission-seat'), 0);
});

test('a failed request releases its slot (#343)', async () => {
  const config = withFleetProfile(base, profile('admission-error', 1));
  await assert.rejects(ask(config, async () => { throw new TypeError('network down'); }, 'x'));
  assert.equal(depth('admission-error'), 0);
  const response = await ask(config, async () => reply(), 'y');
  assert.equal(response.choices[0].message.content, 'ok');
});

test('a config without a fleet profile is not admission-limited (#343)', async () => {
  const endpoint = gatedEndpoint();
  const calls = [ask(base, endpoint.fetchImpl, 'a'), ask(base, endpoint.fetchImpl, 'b')];
  await endpoint.until(() => endpoint.stats().inFlight === 2);
  endpoint.gates.splice(0).forEach((open) => open());
  await Promise.all(calls);
});
