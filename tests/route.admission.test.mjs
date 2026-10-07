import assert from 'node:assert/strict';
import test from 'node:test';
import { routeFailure } from '../src/llm/openai.mjs';
import { chooseRoute } from '../src/lib/route.mjs';
import { acquire, configure, depth } from '../src/runtime/admission.mjs';

const profile = (id, model = 'owner/shared') => ({ id, base_url: `https://${id}.example.invalid/v1`, model,
  provider: 'vllm', context_max: 32768, concurrency: 1, hardware: 'test-gpu', task_class: ['fix'], notes: '' });
const capabilities = { capabilities: [] };
const samples = (model, verdicts) => verdicts.map((verdict, index) => ({
  model, task_class: 'fix', effort: 'h', session: `${model.replace('/', '-')}-${index}`,
  evaluation: { session: `${model.replace('/', '-')}-${index}`, verdict, difficulty: 2, again: true, minutes: 10 },
}));
const select = (fleet, options = {}) => chooseRoute({ fleet, capabilities, taskClass: 'fix', difficulty: 2, ...options });

test('admission depth breaks a tie between equally scored eval-history routes (#344)', () => {
  const fleet = { profiles: [profile('route-a'), profile('route-b')] };
  const records = samples('owner/shared', ['accept', 'accept', 'accept']);
  assert.equal(select(fleet, { records, queueDepth: () => 0 }).profile.id, 'route-a');
  const busy = { 'route-a': 2, 'route-b': 0 };
  const choice = select(fleet, { records, queueDepth: (id) => busy[id] });
  assert.equal(choice.profile.id, 'route-b');
  assert.equal(choice.source, 'evals');
});

test('eval history outranks admission depth (#344)', () => {
  const fleet = { profiles: [profile('route-good', 'owner/good'), profile('route-weak', 'owner/weak')] };
  const records = [...samples('owner/good', ['accept', 'accept', 'accept']),
    ...samples('owner/weak', ['accept', 'reject', 'reject'])];
  const busy = { 'route-good': 5, 'route-weak': 0 };
  assert.equal(select(fleet, { records, queueDepth: (id) => busy[id] }).profile.id, 'route-good');
});

test('prior routes consult the live admission queue only after hints, cost, and context fit (#344)', async () => {
  const fleet = { profiles: [profile('route-live-a'), profile('route-live-b')] };
  assert.equal(select(fleet).profile.id, 'route-live-a');
  configure('route-live-a', 1);
  const held = await acquire('route-live-a');
  const controller = new AbortController();
  const queued = acquire('route-live-a', { signal: controller.signal });
  assert.equal(depth('route-live-a'), 1);
  assert.equal(select(fleet).profile.id, 'route-live-b');
  controller.abort();
  await assert.rejects(queued, { name: 'AbortError' });
  held();
  assert.equal(select(fleet).profile.id, 'route-live-a');
});

test('an aborted admission wait is not a route failure, unlike an endpoint timeout (#344)', async () => {
  configure('route-abort', 1);
  const held = await acquire('route-abort');
  const controller = new AbortController();
  const queued = acquire('route-abort', { signal: controller.signal });
  controller.abort();
  const error = await queued.catch((reason) => reason);
  held();
  assert.equal(error.name, 'AbortError');
  assert.equal(routeFailure(error), null);
  assert.equal(routeFailure(new Error('wrapped', { cause: error })), null);
  const timeout = Object.assign(new Error('timed out'), { code: 'ROSTER_LLM_TIMEOUT' });
  assert.deepEqual(routeFailure(timeout), { reason: 'endpoint-timeout' });
});
