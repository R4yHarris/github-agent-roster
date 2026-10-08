import assert from 'node:assert/strict';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { routeFailure } from '../src/llm/openai.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';
import { chooseRoute } from '../src/lib/route.mjs';
import { acquire, configure, depth } from '../src/runtime/admission.mjs';
import { fixture, llmConfig, runBuiltinIssue } from './helpers/builtin.mjs';

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

test('a busy admitted route completes without quarantine, failover, or profile exclusion (#324)', { timeout: 60_000 }, async (t) => {
  const contracts = resolveContractsPath();
  const options = fixture(t);
  options.issue.title = 'feat: Add status';
  const first = { ...profile('admission-integration-first', 'owner/first'), task_class: ['feat'] };
  const alternate = { ...profile('admission-integration-alternate', 'owner/alternate'),
    context_max: 65536, task_class: ['feat'] };
  mkdirSync(path.join(options.target, '.roster'));
  writeFileSync(path.join(options.target, '.roster', 'fleet.yml'),
    formatFleet({ profiles: [first, alternate] }));
  configure(first.id, first.concurrency);
  const release = await acquire(first.id);
  const controller = new AbortController();
  t.after(() => { release(); controller.abort(); });
  let waiting;
  const queued = new Promise((resolve) => { waiting = resolve; });
  const events = [];
  const logs = [];
  const requests = [];
  let selections = 0;
  const run = runBuiltinIssue(42, {
    ...options, env: { ...options.env, GITHUB_AGENT_CONTRACTS: contracts },
    config: llmConfig, autoModel: true,
    signal: controller.signal,
    metricsLoader: () => { selections += 1; return []; },
    log: (line) => logs.push(line),
    onRunEvent: (event) => {
      events.push(event);
      if (event.type === 'waiting' && event.queued) waiting(event);
    },
    fetchImpl: async (url, request) => {
      const body = JSON.parse(request.body);
      requests.push({ url: String(url), model: body.model });
      const message = requests.length === 1 ? {
        role: 'assistant', content: JSON.stringify({
          title: 'Add status', acceptance_checks: ['node --test exits 0', 'README has a Status section'],
          files_allowed: ['README.md'],
        }),
      } : requests.length === 2 ? {
        role: 'assistant', tool_calls: [{ id: 'status', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({
            path: 'README.md', content: '# Example\n\n## Status\nReady.\n',
          }),
        } }],
      } : { role: 'assistant', content: 'Added status; tests pass.' };
      return Response.json({ choices: [{ finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message }] });
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  try {
    const event = await Promise.race([queued, run.then(() => assert.fail('run completed before admission wait'))]);
    assert.equal(event.depth, 1);
    assert.equal(depth(first.id), 1);
    assert.deepEqual(requests, [], 'the occupied slot must prevent any transport request');
    assert.equal(selections, 1);
    assert.equal(events.some(({ type }) => type === 'route-recovery'), false);
    release();
    const result = await run;
    assert.equal(result.failed, false);
    assert.equal(result.result.mode, 'llm');
    assert.ok(result.result.excellence.pass);
    assert.equal(result.review.verdict, 'pass');
    assert.ok(requests.length >= 3);
    assert.ok(requests.every(({ url, model }) =>
      url === `${first.base_url}/chat/completions` && model === first.model));
    assert.equal(result.route.profile.id, first.id);
    assert.deepEqual(result.routeAttempts, []);
    assert.equal(selections, 2, 'only initial staffing and independent reviewer staffing may select routes');
    assert.equal(events.some(({ type }) => type === 'route-recovery'), false);
    assert.equal(logs.some((line) => /route recovery|quarantine/i.test(line)), false);
    assert.equal(existsSync(path.join(options.target, '.roster', 'runs', 'route-quarantine.json')), false);
    assert.equal(depth(first.id), 0);
  } finally {
    release();
    controller.abort();
    await Promise.allSettled([run]);
  }
});
