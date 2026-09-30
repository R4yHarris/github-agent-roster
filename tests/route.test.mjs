import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { chooseRoute, formatRoute, routeTask } from '../src/lib/route.mjs';

const installation = fileURLToPath(new URL('../', import.meta.url));
const example = readFileSync(join(installation, 'roster.config.example.yml'), 'utf8');
const fast = { id: 'fast', base_url: 'https://fast.example.invalid/v1', model: 'owner/fast',
  provider: 'vllm', context_max: 32768, concurrency: 4, hardware: 'test-gpu', task_class: ['fix'], notes: '' };
const steady = { ...fast, id: 'steady', base_url: 'https://steady.example.invalid/v1',
  model: 'owner/steady', context_max: 65536, concurrency: 1, task_class: ['feat'] };
const fleet = { profiles: [fast, steady] };
const capabilities = { capabilities: [
  { profile_id: 'fast', task_class: 'fix', suggested_difficulty: 5,
    context_max: 32768, concurrency: 4, notes: 'Starting guess.' },
  { model_id: 'owner/steady', task_class: 'fix', suggested_difficulty: 1,
    context_max: 65536, concurrency: 1, notes: 'Starting guess.' },
] };
const samples = (model, n = 3) => Array.from({ length: n }, (_, index) => ({
  model, task_class: 'fix', effort: 'h', session: `${model.replace('/', '-')}-${index}`,
  evaluation: { session: `${model.replace('/', '-')}-${index}`,
    verdict: 'accept', difficulty: 4, again: true, minutes: 10 },
}));
const select = (options = {}) => chooseRoute({
  fleet, capabilities, taskClass: 'fix', difficulty: 3, records: [], ...options,
});

test('three distinct human evaluations win over a stronger capability prior', () => {
  const choice = select({ records: samples('owner/steady') });
  assert.equal(choice.profile.id, 'steady');
  assert.equal(choice.source, 'evals');
  assert.equal(choice.recommendation.n, 3);
  assert.equal(choice.recommendation.acceptRate, 1);
  assert.match(formatRoute(choice, 'fix'), /profile=steady source=evals.*3 distinct human evaluations/);
});

test('zero or two evaluations use a matching prior, not a fabricated benchmark result', () => {
  for (const n of [0, 2]) {
    const choice = select({ records: samples('owner/steady', n) });
    assert.equal(choice.profile.id, 'fast');
    assert.equal(choice.source, 'prior');
    assert.equal(choice.recommendation, null);
    assert.match(choice.reason, /starting guess, not a benchmark/);
  }
  const duplicate = samples('owner/steady', 1)[0];
  assert.equal(select({ records: [duplicate, duplicate, duplicate] }).source, 'prior');
});

test('unregistered models and missing requested profile IDs cannot be selected', () => {
  const choice = select({ records: samples('owner/not-registered', 8) });
  assert.equal(choice.profile.id, 'fast');
  assert.throws(() => select({ profileId: 'not-registered' }), /not found/);
  assert.equal(select({ profileId: 'steady' }), null);
});

test('declared context capacity is tested exactly and an unknown prior cannot satisfy a required limit', () => {
  assert.equal(select({ records: samples('owner/steady'), contextRequired: 65536 }).profile.id, 'steady');
  assert.equal(select({ records: samples('owner/steady'), contextRequired: 65537 }), null);
  const unknown = { ...fast, id: 'default', context_max: 0 };
  const prior = { ...capabilities.capabilities[0], profile_id: 'default', context_max: 65536 };
  const choice = select({ fleet: { profiles: [unknown] }, capabilities: { capabilities: [prior] } });
  assert.equal(choice.profile.context_max, 0);
  assert.match(choice.reason, /capacity remains unknown/);
  assert.equal(select({ fleet: { profiles: [unknown] }, capabilities: { capabilities: [prior] },
    contextRequired: 1 }), null);
});

test('task-class hints precede declared concurrency as a weak deterministic tie-break', () => {
  const other = { ...fast, id: 'wider', model: 'owner/wider', concurrency: 8 };
  const rows = { capabilities: [...capabilities.capabilities,
    { ...capabilities.capabilities[0], profile_id: 'wider' }] };
  assert.equal(select({ fleet: { profiles: [fast, other] }, capabilities: rows }).profile.id, 'wider');
  assert.equal(select({ fleet: { profiles: [fast, { ...other, task_class: ['docs'] }] },
    capabilities: rows }).profile.id, 'fast');
});

test('automatic excellence passes are not human evals, and recorded defects remain rejects', () => {
  const automated = Array.from({ length: 3 }, (_, index) => ({
    model: 'owner/steady', task_class: 'fix', session: `auto-${index}`, excellence: 'pass',
  }));
  assert.equal(select({ records: automated }).source, 'prior');
  const defective = samples('owner/steady');
  defective[0].defects = ['Diff path is protected or outside TASK.md allowed paths: .env'];
  const clean = samples('owner/fast');
  assert.equal(select({ records: [...defective, ...clean] }).profile.id, 'fast');
  assert.equal(defective[0].evaluation.verdict, 'accept');
});

test('CLI recommend prints the same read-only choice as route.mjs without changing default settings', async (t) => {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-route-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  mkdirSync(join(cwd, '.roster', 'runs'), { recursive: true });
  writeFileSync(join(cwd, '.roster', 'fleet.yml'), formatFleet(fleet));
  writeFileSync(join(cwd, '.roster', 'config.yml'), example);
  const evaluations = samples('owner/steady').map(({ model, task_class, effort, evaluation }) =>
    ({ ...evaluation, model, task_class, effort }));
  writeFileSync(join(cwd, '.roster', 'evals.jsonl'), evaluations.map((row) => JSON.stringify(row)).join('\n') + '\n');
  const joined = samples('owner/steady');
  const choice = await routeTask({ cwd, installationRoot: installation, taskClass: 'fix',
    difficulty: 3, records: joined });
  const result = spawnSync(process.execPath, [join(installation, 'src', 'cli.mjs'),
    'recommend', '--task-class', 'fix', '--difficulty', '3'], {
    cwd, encoding: 'utf8', timeout: 10_000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, formatRoute(choice, 'fix', parseConfig(example)));
  assert.equal(readFileSync(join(cwd, '.roster', 'config.yml'), 'utf8'), example);
});
