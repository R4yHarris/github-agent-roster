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
    verdict: 'accept', difficulty: 2, again: true, minutes: 10 },
}));
const select = (options = {}) => chooseRoute({
  fleet, capabilities, taskClass: 'fix', difficulty: 2, records: [], ...options,
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

test('a sufficient dedicated profile beats a cluster profile for an ordinary task', () => {
  const local = { ...fast, id: 'spark', model: 'deepseek-v4.1-flash', base_url: 'http://192.168.1.48:8888/v1',
    hardware: 'DGX Spark', context_max: 1048576, concurrency: 1, task_class: ['feat'] };
  const remote = { ...fast, id: 'aperture-qwen', model: 'qwen3.8-27b',
    base_url: 'https://aperture.example/v1', hardware: 'RTX 6000', context_max: 262144, concurrency: 8,
    task_class: ['feat'] };
  const rows = { capabilities: ['spark', 'aperture-qwen'].map((profile_id) => ({
    profile_id, task_class: 'feat', suggested_difficulty: 3, context_max: 262144, concurrency: 1,
    notes: 'Starting guess.',
  })) };
  assert.equal(select({ fleet: { profiles: [local, remote] }, capabilities: rows, taskClass: 'feat' }).profile.id,
    'aperture-qwen');
});

test('task-class hints precede declared concurrency as a weak deterministic tie-break', () => {
  const other = { ...fast, id: 'wider', model: 'owner/wider', concurrency: 8 };
  const rows = { capabilities: [...capabilities.capabilities,
    { ...capabilities.capabilities[0], profile_id: 'wider' }] };
  assert.equal(select({ fleet: { profiles: [fast, other] }, capabilities: rows }).profile.id, 'wider');
  assert.equal(select({ fleet: { profiles: [fast, { ...other, task_class: ['docs'] }] },
    capabilities: rows }).profile.id, 'fast');
});

test('current-run exclusions force routing to a different eligible profile', () => {
  const other = { ...fast, id: 'wider', model: 'owner/wider', concurrency: 8 };
  const rows = { capabilities: [...capabilities.capabilities,
    { ...capabilities.capabilities[0], profile_id: 'wider' }] };
  assert.equal(select({ fleet: { profiles: [fast, other] }, capabilities: rows }).profile.id, 'wider');
  assert.equal(select({ fleet: { profiles: [fast, other] }, capabilities: rows,
    excludedProfileIds: ['wider'] }).profile.id, 'fast');
  assert.equal(select({ fleet: { profiles: [fast, other] }, capabilities: rows,
    excludedProfileIds: ['wider', 'fast'] }), null);
  assert.throws(() => select({ excludedProfileIds: ['fast', 'fast'] }), /distinct opaque identifiers/);
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
    difficulty: 2, records: joined });
  const result = spawnSync(process.execPath, [join(installation, 'src', 'cli.mjs'),
    'recommend', '--task-class', 'fix', '--difficulty', '2'], {
    cwd, encoding: 'utf8', timeout: 10_000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, formatRoute(choice, 'fix', parseConfig(example)));
  assert.match(result.stdout, /origin=local-human-evaluations n=3 accepted=3 rejected=0/);
  assert.match(result.stdout, /latest=unknown age-days=unknown warning=none/);
  assert.equal(readFileSync(join(cwd, '.roster', 'config.yml'), 'utf8'), example);
});

test('ceilings gate both human evidence and prior fallbacks, including explicit profiles', () => {
  const accepted = samples('owner/fast');
  assert.equal(select({ records: accepted, difficulty: 3 }).source, 'prior');
  assert.equal(select({ records: accepted, difficulty: 4 }), null);
  const rejected = samples('owner/fast', 1);
  rejected[0].evaluation.verdict = 'reject';
  assert.equal(select({ records: rejected }), null);
  assert.equal(select({ records: rejected, profileId: 'fast' }), null);
  assert.equal(select({ records: rejected, difficulty: 1 }).source, 'prior');
  assert.equal(select({ records: [], difficulty: 5 }).profile.id, 'fast');
});

test('routing uses only the requested seat evidence and never borrows another seat capacity', () => {
  const planner = samples('owner/fast', 1).map((record) => ({ ...record, seat: 'planner',
    evaluation: { ...record.evaluation, verdict: 'reject' } }));
  assert.equal(select({ records: planner }).profile.id, 'fast');
  assert.equal(select({ records: planner, seat: 'planner' }), null);
  const plannerAccepts = samples('owner/steady').map((record) => ({ ...record, seat: 'planner' }));
  assert.equal(select({ records: plannerAccepts }).source, 'prior');
  assert.equal(select({ records: plannerAccepts, seat: 'planner' }).profile.id, 'steady');
  assert.throws(() => select({ seat: '' }), /Routing seat/);
});

test('a defect-backed acceptance lowers the ceiling even when a strong prior remains', () => {
  const records = samples('owner/fast');
  records[2].defects = ['Protected path touched'];
  assert.equal(select({ records }), null);
  records[2].defects = [];
  records[2].evaluation.verdict = 'rework';
  assert.equal(select({ records, difficulty: 3 }), null);
});


test('higher-level accepts cannot skip earning the current ceiling', () => {
  const records = samples('owner/fast').map((record) => ({ ...record,
    evaluation: { ...record.evaluation, difficulty: 4 } }));
  assert.equal(select({ records, difficulty: 4, profileId: 'fast' }), null);
});
