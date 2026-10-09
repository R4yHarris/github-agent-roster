import test from 'node:test';
import assert from 'node:assert/strict';
import { cp, mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { openProvenanceStore, LOG_DIRNAME } from '../src/lib/provenance-store.mjs';
import { identityHash, resolveRepoIdentity } from '../src/lib/repo-identity.mjs';
import { buildProvenanceRecord, createProvenanceStore, storeRecordId } from '../src/lib/provenance-api.mjs';
import { runHistory } from '../src/lib/history-cli.mjs';

const REPO = identityHash({ gitCommonDir: 'common-a', remoteUrl: 'sentinel-a' });

test('machine history survives linked-worktree cleanup and actual checkout deletion/reclone without copying records', async (t) => {
  const dir = await scratch(t);
  const origin = path.join(dir, 'origin');
  const checkout = path.join(dir, 'checkout');
  const linked = path.join(dir, 'linked');
  const reclone = path.join(dir, 'reclone');
  const root = path.join(dir, 'machine', 'provenance');
  const git = (...args) => execFileSync('git', args, { stdio: 'pipe' });
  git('init', '-q', origin);
  git('-C', origin, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '-q', '--allow-empty', '-m', 'seed');
  git('clone', '-q', '--no-local', origin, checkout);
  git('-C', checkout, 'worktree', 'add', '-q', '--detach', linked);
  const identity = await resolveRepoIdentity({ repoRoot: linked });
  const now = 1735689600000;
  const record = {
    ...buildProvenanceRecord({ runId: 'cleanup-run', sessionId: 'cleanup-session', event: 'cancellation',
      payload: { outcome: 'cancelled' } }, { repoIdentity: identity, now }),
    id: storeRecordId(identity, 'cleanup-run', 'cleanup-session', 'cancellation', 'raw-history'),
    version: 1, section: 'raw-history',
  };
  await openProvenanceStore(root, { clock: () => now }).appendRecord(record);
  const recordPath = path.join(root, LOG_DIRNAME, `${record.id}.json`);
  const bytes = await readFile(recordPath);
  git('-C', checkout, 'worktree', 'remove', linked);
  await rm(checkout, { recursive: true, force: true });
  git('clone', '-q', '--no-local', origin, reclone);
  const reader = createProvenanceStore({ root, repoRoot: reclone });
  assert.equal(await reader.identity(), identity);
  const records = await reader.query({ runId: 'cleanup-run' });
  assert.equal(records.length, 1);
  assert.equal(records[0].event, 'cancellation');
  assert.equal(records[0].payload.outcome, 'cancelled');
  assert.equal(records[0].createdAt, record.createdAt);
  assert.deepEqual(await readFile(recordPath), bytes);
  git('-C', reclone, 'remote', 'set-url', 'origin', 'https://example.invalid/different/repository.git');
  assert.deepEqual(await createProvenanceStore({ root, repoRoot: reclone }).query(), []);
  assert.deepEqual(await readFile(recordPath), bytes);
});

async function scratch(t) {
  const dir = await mkdtemp(path.join(process.cwd(), '.history-robust-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return dir;
}

async function populate(root) {
  const store = openProvenanceStore(root);
  await store.appendRecord({
    id: 'record-1', version: 1, runId: 'r1', sessionId: 's1', repoIdentity: REPO,
    createdAt: '2024-01-01T00:00:00.000Z', payload: { issue: 7, seat: 'coder', model: 'model-a', outcome: 'pass' },
  });
}

test('non-object JSON roots are skipped while valid records still render', async (t) => {
  const root = path.join(await scratch(t), 'store');
  await populate(root);
  for (const [name, body] of [['null.json', 'null'], ['number.json', '42'], ['array.json', '[]'],
    ['string.json', '"text"'], ['garbage.json', '\u0000\u00ff\u0001']]) {
    await writeFile(path.join(root, LOG_DIRNAME, name), body);
  }
  const list = await runHistory(['list', '--store', root]);
  assert.match(list, /Skipped 5 unreadable/);
  assert.match(list, /session=s1.*run=r1/);
  const json = JSON.parse(await runHistory(['list', '--store', root, '--format', 'json']));
  assert.equal(json.meta.skippedCount, 5);
  assert.deepEqual(json.records.map((record) => record.runId), ['r1']);
});

test('a store moved away from a deleted or re-cloned checkout stays readable', async (t) => {
  const dir = await scratch(t);
  const original = path.join(dir, 'old-checkout', '.git', 'roster', 'provenance');
  await populate(original);
  const moved = path.join(dir, 'reclone', '.git', 'roster', 'provenance');
  await cp(original, moved, { recursive: true });
  await rm(path.join(dir, 'old-checkout'), { recursive: true, force: true });
  // Identity is a derived hash, never a checkout path, so nothing points back at the old location.
  assert.match(REPO, /^[a-z0-9]+-[a-f0-9]{64}$/);
  assert.match(await runHistory(['list', '--store', moved]), /session=s1.*run=r1/);
  assert.match(await runHistory(['show', 'r1', '--store', moved]), /session=s1.*run=r1/);
  const json = JSON.parse(await runHistory(['list', '--store', moved, '--format', 'json']));
  assert.equal(json.records[0].repository, REPO);
});

test('mixed and trailing separators in --store resolve to the same store', async (t) => {
  const dir = await scratch(t);
  const root = path.join(dir, 'store');
  await populate(root);
  const variants = [root, `${root}${path.sep}`, path.join(dir, '.', 'nested', '..', 'store')];
  if (process.platform === 'win32') variants.push(root.replace(/\\/g, '/'));
  for (const variant of variants) {
    const json = JSON.parse(await runHistory(['list', '--store', variant, '--format', 'json']));
    assert.equal(path.resolve(json.meta.storeRoot), path.resolve(root), variant);
    assert.deepEqual(json.records.map((record) => record.runId), ['r1'], variant);
  }
});

test('repo-local .roster state is never merged into the default machine history', async (t) => {
  const dir = await scratch(t);
  const store = path.join(dir, '.git', 'roster', 'provenance');
  await populate(store);
  const roster = path.join(dir, '.roster', 'runs');
  await mkdir(roster, { recursive: true });
  const decoy = JSON.stringify({ id: 'decoy', version: 1, runId: 'decoy-run', sessionId: 'decoy-session',
    repoIdentity: REPO, event: 'completed', section: 'raw-history', createdAt: Date.now() });
  await writeFile(path.join(roster, 'runs.jsonl'), `${decoy}\n`);
  await writeFile(path.join(dir, '.roster', 'decoy.json'), decoy);
  const run = async () => ({ stdout: '.git\n' });
  const json = JSON.parse(await runHistory(['list', '--format', 'json'], { cwd: dir, run }));
  assert.deepEqual(json.records.map((record) => record.runId), ['r1']);
  assert.equal(json.meta.skippedCount, 0);
  await assert.rejects(runHistory(['show', 'decoy-run'], { cwd: dir, run }), /No history record found/);
});
