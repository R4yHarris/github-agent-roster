import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

import {
  CLEAN_TARGETS,
  formatProvenanceReport,
  pruneProvenance,
  resolveCleanTarget,
  runClean,
} from '../src/lib/clean-ops.mjs';
import { acquireRepoLock } from '../src/lib/repo-locks.mjs';
import { LOCK_NAME, SEGMENT_NAME, openProvenanceStore } from '../src/lib/provenance-store.mjs';

const CLI = path.join(import.meta.dirname, '..', 'src', 'cli.mjs');

function makeRepo(t, name = 'repo', origin = 'https://sentinel.example/repo.git') {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'clean-targets-'));
  t.after(() => fsp.rm(tmp, { recursive: true, force: true }).catch(() => {}));
  const repoRoot = path.join(tmp, name);
  fs.mkdirSync(repoRoot, { recursive: true });
  execFileSync('git', ['init', '--quiet'], { cwd: repoRoot });
  execFileSync('git', ['remote', 'add', 'origin', origin], { cwd: repoRoot });
  const machineRoot = path.join(tmp, 'machine');
  fs.mkdirSync(machineRoot, { recursive: true });
  return { tmp, repoRoot, machineRoot };
}

function seed(root, entries) {
  for (const entry of entries) {
    const file = path.join(root, entry);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, 'x');
  }
}

async function seedStore(root, records) {
  const store = openProvenanceStore(root);
  for (const record of records) await store.appendRecord({ version: 1, ...record });
  return store;
}

test('targets are the four explicit #197 targets', () => {
  assert.deepEqual(CLEAN_TARGETS, ['issue', 'repo', 'machine-history', 'curated-memory']);
});

test('cleaning one issue leaves sibling issues, repo state and the worktree checkout untouched', async (t) => {
  const { repoRoot, machineRoot } = makeRepo(t);
  const one = await resolveCleanTarget('issue', { repoRoot, machineRoot, issue: 1 });
  const two = await resolveCleanTarget('issue', { repoRoot, machineRoot, issue: '2' });
  const repo = await resolveCleanTarget('repo', { repoRoot, machineRoot });
  assert.equal(one.handle.scope, 'worktree');
  assert.notEqual(one.handle.root, two.handle.root);
  seed(one.handle.root, ['runs/r1/run.log', 'memory.jsonl']);
  seed(two.handle.root, ['runs/r2/run.log']);
  seed(repo.handle.root, ['cache/a']);
  seed(repoRoot, ['.worktrees/issue-1/src/app.mjs']);

  const preview = await runClean(one.handle, { exclude: one.exclude });
  assert.deepEqual(preview.included.map(({ name }) => name), ['memory.jsonl', 'runs']);
  const done = await runClean(one.handle, { exclude: one.exclude, execute: true, yes: true });
  assert.deepEqual(done.removed.map(({ target }) => path.basename(target)), ['memory.jsonl', 'runs']);
  assert.deepEqual(fs.readdirSync(one.handle.root), []);
  assert.ok(fs.existsSync(path.join(two.handle.root, 'runs', 'r2', 'run.log')));
  assert.ok(fs.existsSync(path.join(repo.handle.root, 'cache', 'a')));
  assert.ok(fs.existsSync(path.join(repoRoot, '.worktrees', 'issue-1', 'src', 'app.mjs')));
  assert.ok(fs.existsSync(path.join(repoRoot, '.git', 'HEAD')));
  // Repeating the clean is a no-op, not an error.
  assert.deepEqual((await runClean(one.handle, { execute: true, yes: true })).removed, []);
});

test('repo reset keeps identity, schema, issue state, machine history and other repositories', async (t) => {
  const { repoRoot, machineRoot, tmp } = makeRepo(t);
  const other = path.join(tmp, 'other');
  fs.mkdirSync(other);
  execFileSync('git', ['init', '--quiet'], { cwd: other });
  execFileSync('git', ['remote', 'add', 'origin', 'https://sentinel.example/other.git'], { cwd: other });
  const repo = await resolveCleanTarget('repo', { repoRoot, machineRoot });
  const otherRepo = await resolveCleanTarget('repo', { repoRoot: other, machineRoot });
  const issue = await resolveCleanTarget('issue', { repoRoot, machineRoot, issue: 7 });
  assert.notEqual(repo.handle.root, otherRepo.handle.root);
  seed(repo.handle.root, ['identity', 'schema.json', 'cache/a', 'notes.json']);
  seed(issue.handle.root, ['runs/r/run.log']);
  seed(otherRepo.handle.root, ['cache/b']);
  const history = path.join(repoRoot, '.git', 'roster', 'provenance');
  await seedStore(history, [{ id: 'raw-1', section: 'raw-history' }]);

  const done = await runClean(repo.handle, { exclude: repo.exclude, execute: true, yes: true });
  assert.deepEqual(done.removed.map(({ target }) => path.basename(target)), ['cache', 'notes.json']);
  assert.deepEqual(fs.readdirSync(repo.handle.root).sort(), ['identity', 'schema.json', 'worktrees']);
  assert.ok(fs.existsSync(path.join(issue.handle.root, 'runs', 'r', 'run.log')));
  assert.ok(fs.existsSync(path.join(otherRepo.handle.root, 'cache', 'b')));
  assert.deepEqual((await openProvenanceStore(history).readAll()).records.map(({ id }) => id), ['raw-1']);
});

test('machine-history prune is selective by repository and age and keeps curated memory', async (t) => {
  const { tmp } = makeRepo(t);
  const store = path.join(tmp, 'provenance');
  await seedStore(store, [
    { id: 'a-old', section: 'raw-history', repoIdentity: 'A', createdAt: 1_000 },
    { id: 'a-new', section: 'raw-history', repoIdentity: 'A', createdAt: 9_000 },
    { id: 'a-compact', section: 'compaction', repoIdentity: 'A', createdAt: 1_000 },
    { id: 'b-old', section: 'raw-history', repoIdentity: 'B', createdAt: 1_000 },
    { id: 'a-memory', section: 'curated-memory', repoIdentity: 'A', createdAt: 1_000 },
  ]);
  const options = { target: 'machine-history', repoIdentity: 'A', before: 5_000 };
  const preview = await pruneProvenance(store, options);
  assert.equal(preview.dryRun, true);
  assert.deepEqual(preview.records, ['a-compact', 'a-old']);
  assert.match(formatProvenanceReport(preview), /mode: dry-run[\s\S]*records: 2[\s\S]*kept: 3/);
  const done = await pruneProvenance(store, { ...options, execute: true, yes: true });
  assert.deepEqual(done.removed, preview.records);
  const left = (await openProvenanceStore(store).readAll()).records.map(({ id }) => id);
  assert.deepEqual(left, ['a-memory', 'a-new', 'b-old']);
  // The segment index no longer carries pruned records.
  const segment = fs.readFileSync(path.join(store, SEGMENT_NAME), 'utf8');
  assert.doesNotMatch(segment, /a-old|a-compact/);
  assert.match(segment, /a-new/);
  assert.deepEqual((await pruneProvenance(store, { ...options, execute: true, yes: true })).removed, []);
});

test('curated-memory reset keeps raw provenance', async (t) => {
  const { tmp } = makeRepo(t);
  const store = path.join(tmp, 'provenance');
  await seedStore(store, [
    { id: 'raw', section: 'raw-history' },
    { id: 'legacy' },
    { id: 'memory', section: 'curated-memory' },
  ]);
  const done = await pruneProvenance(store, { target: 'curated-memory', execute: true, yes: true });
  assert.deepEqual(done.removed, ['memory']);
  assert.deepEqual((await openProvenanceStore(store).readAll()).records.map(({ id }) => id), ['legacy', 'raw']);
});

test('provenance prune refuses unconfirmed, unsafe, mismatched and locked requests', async (t) => {
  const { tmp, repoRoot } = makeRepo(t);
  const store = path.join(tmp, 'provenance');
  await seedStore(store, [{ id: 'raw', section: 'raw-history' }]);
  const code = (expected) => (error) => error.code === expected;
  await assert.rejects(pruneProvenance(store, { target: 'machine-history', execute: true }), code('CLEAN_OPS_CONFIRMATION_REQUIRED'));
  await assert.rejects(pruneProvenance(store, { target: 'repo' }), code('CLEAN_OPS_SCOPE_MISMATCH'));
  await assert.rejects(pruneProvenance(store, { target: 'nope' }), code('CLEAN_OPS_SCOPE_MISMATCH'));
  await assert.rejects(pruneProvenance(store, { target: 'machine-history', before: 'soon' }), code('CLEAN_OPS_SCOPE_MISMATCH'));
  await assert.rejects(pruneProvenance('relative/store', { target: 'machine-history' }), code('CLEAN_OPS_FORBIDDEN_PATH'));
  await assert.rejects(pruneProvenance(os.homedir(), { target: 'machine-history' }), code('CLEAN_OPS_FORBIDDEN_PATH'));
  await assert.rejects(pruneProvenance(repoRoot, { target: 'machine-history', repoRoot }), code('CLEAN_OPS_FORBIDDEN_PATH'));
  const lock = await acquireRepoLock(LOCK_NAME, { lockRoot: store, holder: 'writer', waitMs: 0 });
  try {
    await assert.rejects(pruneProvenance(store, { target: 'machine-history', execute: true, yes: true }),
      code('CLEAN_OPS_LOCK_HELD'));
  } finally {
    await lock.release();
  }
  assert.deepEqual((await openProvenanceStore(store).readAll()).records.map(({ id }) => id), ['raw']);
});

test('directory targets refuse bad issue numbers and provenance targets', async (t) => {
  const { repoRoot, machineRoot } = makeRepo(t);
  for (const issue of [undefined, 0, -1, 1.5, '01', 'x']) {
    await assert.rejects(resolveCleanTarget('issue', { repoRoot, machineRoot, issue }),
      (error) => error.code === 'CLEAN_OPS_SCOPE_MISMATCH');
  }
  for (const target of ['machine-history', 'curated-memory', 'all']) {
    await assert.rejects(resolveCleanTarget(target, { repoRoot, machineRoot }),
      (error) => error.code === 'CLEAN_OPS_SCOPE_MISMATCH');
  }
});

test('roster clean --target previews and prunes only the named target', async (t) => {
  const { tmp, repoRoot, machineRoot } = makeRepo(t);
  const store = path.join(tmp, 'provenance');
  await seedStore(store, [{ id: 'raw', section: 'raw-history' }, { id: 'memory', section: 'curated-memory' }]);
  const cli = (...args) => execFileSync(process.execPath, [CLI, 'clean', ...args], {
    cwd: repoRoot, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, ROSTER_STATE_ROOT: machineRoot },
  });
  assert.match(cli('--target', 'curated-memory', '--store', store), /mode: dry-run[\s\S]*records: 1\n  - record memory/);
  assert.match(cli('--target', 'curated-memory', '--store', store, '--execute', '--yes'), /removed: 1/);
  assert.deepEqual((await openProvenanceStore(store).readAll()).records.map(({ id }) => id), ['raw']);
  assert.match(cli('--target', 'issue', '--issue', '3'), /mode: dry-run[\s\S]*scope: worktree[\s\S]*records: 0/);
  assert.match(cli('--target', 'repo'), /scope: repo\nstate root: .*machine[\s\S]*records: 0/);
  for (const bad of [['--target', 'repo', '--issue', '3'], ['--target', 'issue', '--store', store],
    ['--target', 'repo', '--scope', 'repo']]) {
    assert.throws(() => cli(...bad), (error) => /Use roster clean/.test(String(error.stderr)));
  }
});
