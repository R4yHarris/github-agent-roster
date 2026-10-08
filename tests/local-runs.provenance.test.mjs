import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { provenanceStoreForRun } from '../src/lib/local-runs.mjs';
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';

test('run provenance honors machine root and survives checkout deletion, including linked worktrees', async (t) => {
  const root = await mkdtemp(path.join(tmpdir(), 'local-provenance-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const machine = path.join(root, 'machine');
  const checkout = path.join(root, 'checkout');
  const linked = path.join(root, 'linked');
  await mkdir(machine);
  await mkdir(checkout);
  await mkdir(linked);
  await mkdir(path.join(checkout, '.git'));
  await writeFile(path.join(linked, '.git'), `gitdir: ${path.join(checkout, '.git', 'worktrees', 'linked')}\n`);
  const run = async (_program, args) => {
    if (args.includes('--git-common-dir')) return { stdout: `${path.join(checkout, '.git')}\n` };
    if (args.includes('remote.origin.url')) return { stdout: 'https://example.invalid/fixture/project.git\n' };
    throw new Error(`Unexpected fixture git command: ${args.join(' ')}`);
  };
  const env = { ROSTER_STATE_ROOT: machine };
  const mainStore = provenanceStoreForRun({ repoRoot: checkout, env, run });
  const linkedStore = provenanceStoreForRun({ repoRoot: linked, env, run });
  assert.equal(mainStore.root, path.join(machine, 'provenance'));
  assert.equal(linkedStore.root, mainStore.root);
  assert.equal(await mainStore.identity(), await linkedStore.identity());
  await linkedStore.recordEvent({
    runId: 'fixture-run', sessionId: 'fixture-session', event: 'completed',
    payload: { summary: 'Completed disposable fixture' },
  });
  assert.equal((await mainStore.query({})).length, 1);
  await rm(checkout, { recursive: true });
  await rm(linked, { recursive: true });
  const persisted = await openProvenanceStore(path.join(machine, 'provenance')).readAll();
  assert.equal(persisted.records.length, 1);
  assert.equal(persisted.records[0].event, 'completed');
  assert.deepEqual(persisted.skipped, []);
});

test('invalid machine-root configuration is explicit instead of silently disabling provenance', () => {
  assert.throws(() => provenanceStoreForRun({
    repoRoot: 'fixture', env: { ROSTER_STATE_ROOT: '' },
  }), /must be a non-empty path/);
  assert.equal(provenanceStoreForRun({ env: { ROSTER_STATE_ROOT: '' } }), null);
});
