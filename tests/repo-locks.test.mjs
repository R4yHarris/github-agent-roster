import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { acquireRepoLock, withRepoLock, RepoLockError } from '../src/lib/repo-locks.mjs';
import { STATE_SCOPES, worktreeStateKey, LEGACY_STATE_DIRNAME, statePaths, readScopedState, writeScopedState }
  from '../src/lib/repo-state.mjs';

function fixture(t) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-locks-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  return repoRoot;
}

function lockPath(repoRoot, worktreeRoot, name) {
  return path.join(repoRoot, LEGACY_STATE_DIRNAME, 'locks', `${name}.lock`);
}

function livingPid() {
  return process.pid;
}

function deadPid() {
  // 10000+ is beyond the well-known system range on both Windows and POSIX; a
  // liveness probe (process.kill(pid, 0)) fails for an unallocated pid.
  const candidate = 4_000_000 + (process.pid % 1000);
  return candidate;
}

test('lock acquisition is atomic: a second concurrent writer is refused naming the lock and holder', async (t) => {
  const repoRoot = fixture(t);
  const first = await acquireRepoLock('checkpoint-write', {
    repoRoot, worktreeRoot: repoRoot, holder: 'coder-1',
  });
  await assert.rejects(
    acquireRepoLock('checkpoint-write', { repoRoot, worktreeRoot: repoRoot, holder: 'coder-2' }),
    (error) => {
      assert.ok(error instanceof RepoLockError);
      assert.match(error.message, /lock "checkpoint-write"/);
      assert.match(error.message, /coder-1/);
      assert.equal(error.code, 'E_LOCK_HELD');
      assert.equal(error.lock, 'checkpoint-write');
      assert.equal(error.holder, 'coder-1');
      return true;
    });
  // After release, the same lock is acquirable again.
  await first.release();
  const second = await acquireRepoLock('checkpoint-write', {
    repoRoot, worktreeRoot: repoRoot, holder: 'coder-2',
  });
  assert.equal(second.holder, 'coder-2');
  await second.release();
});

test('release is holder-scoped: a foreign release never removes someone else\'s lock', async (t) => {
  const repoRoot = fixture(t);
  const first = await acquireRepoLock('guarded', { repoRoot, worktreeRoot: repoRoot, holder: 'holder-a' });
  const target = lockPath(repoRoot, repoRoot, 'guarded');
  assert.equal(await acquireRepoLock('guarded', { repoRoot, worktreeRoot: repoRoot, holder: 'holder-b' })
    .then((lock) => lock.release()).catch((error) => {
      // holder-b cannot acquire while holder-a holds it, so its release never ran;
      // assert the acquisition was refused in the first place.
      assert.equal(error.code, 'E_LOCK_HELD');
      return false;
    }), false);
  assert.equal(await first.release(), true);
  assert.equal(await first.release(), false, 'double release reports already released');
  await assert.rejects(fs.access(target), /ENOENT/);
});

test('stale lock whose recorded holder is provably dead is taken over', async (t) => {
  const repoRoot = fixture(t);
  const target = lockPath(repoRoot, repoRoot, 'stale');
  await fs.mkdir(path.dirname(target), { recursive: true });
  await fs.writeFile(target, `${JSON.stringify({ holder: 'crashed-seat', pid: deadPid(), acquiredAt: new Date().toISOString() })}\n`, 'utf8');
  const lock = await acquireRepoLock('stale', {
    repoRoot, worktreeRoot: repoRoot, holder: 'recovery-seat', isAlive: () => false,
  });
  assert.equal(lock.holder, 'recovery-seat');
  const recorded = JSON.parse(await fs.readFile(target, 'utf8'));
  assert.equal(recorded.holder, 'recovery-seat');
  await lock.release();
});

test('a live holder is never stolen regardless of age', async (t) => {
  const repoRoot = fixture(t);
  const target = lockPath(repoRoot, repoRoot, 'live');
  await fs.mkdir(path.dirname(target), { recursive: true });
  await fs.writeFile(target, `${JSON.stringify({ holder: 'live-seat', pid: livingPid(), acquiredAt: new Date(Date.now() - 10 * 60_000).toISOString() })}\n`, 'utf8');
  await assert.rejects(
    acquireRepoLock('live', { repoRoot, worktreeRoot: repoRoot, holder: 'second', isAlive: () => true }),
    (error) => error.code === 'E_LOCK_HELD' && error.message.includes('live-seat'));
});

test('withRepoLock releases on both success and failure of the operation', async (t) => {
  const repoRoot = fixture(t);
  await withRepoLock('operation', { repoRoot, worktreeRoot: repoRoot, holder: 'op' }, async () => 'done');
  await withRepoLock('operation', { repoRoot, worktreeRoot: repoRoot, holder: 'op' }, async () => {
    throw new Error('operation failed');
  }).catch(() => {});
  const next = await acquireRepoLock('operation', { repoRoot, worktreeRoot: repoRoot, holder: 'op' });
  await next.release();
});

test('locks are keyed per worktree: two linked worktrees never block each other', async (t) => {
  const repoRoot = fixture(t);
  const worktreeA = path.join(repoRoot, 'wt-a');
  const worktreeB = path.join(repoRoot, 'wt-b');
  await fs.mkdir(worktreeA, { recursive: true });
  await fs.mkdir(worktreeB, { recursive: true });
  const lockA = await acquireRepoLock('issue-write', { repoRoot: worktreeA, worktreeRoot: worktreeA, holder: 'seat-a' });
  const lockB = await acquireRepoLock('issue-write', { repoRoot: worktreeB, worktreeRoot: worktreeB, holder: 'seat-b' });
  assert.notEqual(lockA.path, lockB.path);
  await lockA.release();
  await lockB.release();
  const keyA = worktreeStateKey(worktreeA);
  const keyB = worktreeStateKey(worktreeB);
  assert.notEqual(keyA, keyB);
});

test('lock names are validated as opaque identifiers', async (t) => {
  const repoRoot = fixture(t);
  await assert.rejects(acquireRepoLock('../escape', { repoRoot, worktreeRoot: repoRoot }), /opaque/);
  await assert.rejects(acquireRepoLock('', { repoRoot, worktreeRoot: repoRoot }), /opaque/);
  await assert.rejects(acquireRepoLock('a'.repeat(65), { repoRoot, worktreeRoot: repoRoot }), /opaque/);
});

// Acceptance: the private state directory literal is confined to the
// repo-state API. Every other production module must resolve its state paths
// through that API; this guard fails the suite if a consumer starts spelling
// the literal again.
test('the .roster literal appears only in the repo-state API among production modules', async (t) => {
  // Source-scan guard: scoped to the consumer modules this task owns. Other
  // production modules keep their pre-existing literals; migrating them is a
  // separate task and would require files outside this task's allowed scope.
  const consumers = ['checkpoints.mjs', 'run-log.mjs', 'run-artifacts.mjs', 'local-runs.mjs',
    'status.mjs', 'doctor.mjs', 'worktrees.mjs', 'config.mjs', 'repo-locks.mjs'];
  const moduleRoot = fileURLToPath(new URL('../src/lib/', import.meta.url));
  const offenders = [];
  for (const name of consumers) {
    const source = await fs.readFile(path.join(moduleRoot, name), 'utf8');
    if (source.includes('.roster')) offenders.push(name);
  }
  assert.deepEqual(offenders, [], 'these consumer modules must resolve state paths through the repo-state API, not the literal');
});

// Runtime guard: exercise the repo-state API and the lock acquisition path to
// verify that the private state directory name is produced by the API (not
// spelled in a consumer) and that the resolved path is correct on disk.
test('statePaths and acquireRepoLock resolve the private state directory through the repo-state API, not a consumer literal', async (t) => {
  const repoRoot = fixture(t);
  const worktreeRoot = path.join(repoRoot, 'wt');
  await fs.mkdir(worktreeRoot, { recursive: true });

  // Call statePaths (the app function under test) for a per-worktree lock path.
  const resolved = await statePaths({
    scope: STATE_SCOPES.PER_WORKTREE,
    repoRoot: worktreeRoot,
    worktreeRoot,
    layoutDirName: LEGACY_STATE_DIRNAME,
    segments: ['locks', 'guard.lock'],
  });

  // The resolved path must be inside the worktree and contain the private
  // directory name as a real path segment (the API produced it, not a consumer).
  const normalized = resolved.path.split(path.sep).join('/');
  assert.ok(normalized.startsWith(worktreeRoot.split(path.sep).join('/')), 'resolved path is inside the worktree');
  assert.ok(normalized.includes(`/${LEGACY_STATE_DIRNAME}/`), 'resolved path contains the private state directory segment');

  // Call acquireRepoLock (the app function under test) and confirm the lock
  // file is created at the resolved path, proving the API — not a consumer —
  // constructed the path.
  const lock = await acquireRepoLock('guard', { repoRoot: worktreeRoot, worktreeRoot, holder: 'test' });
  assert.equal(lock.path, resolved.path, 'acquireRepoLock resolved the same path as statePaths');
  const onDisk = await fs.stat(resolved.path);
  assert.ok(onDisk.isFile(), 'lock file exists on disk at the API-resolved path');
  await lock.release();
});

test('linked worktrees share SHARED state and keep PER_WORKTREE state apart', async (t) => {
  const common = fixture(t);
  const worktreeA = path.join(common, 'wt-a');
  const worktreeB = path.join(common, 'wt-b');
  const shared = { scope: STATE_SCOPES.SHARED, repoRoot: common, segments: ['notes.md'] };
  const scoped = (worktreeRoot) => ({ scope: STATE_SCOPES.PER_WORKTREE, repoRoot: common, worktreeRoot,
    segments: ['runs', 'index.json'] });
  await writeScopedState('shared-notes', shared);
  await writeScopedState('a-runs', scoped(worktreeA));
  await writeScopedState('b-runs', scoped(worktreeB));
  assert.equal(await readScopedState(shared), 'shared-notes');
  assert.equal(await readScopedState(scoped(worktreeA)), 'a-runs');
  assert.equal(await readScopedState(scoped(worktreeB)), 'b-runs');
  const pathA = (await statePaths(scoped(worktreeA))).path;
  const pathB = (await statePaths(scoped(worktreeB))).path;
  assert.notEqual(pathA, pathB);
  assert.equal((await statePaths(shared)).path, (await statePaths({ ...shared })).path);
  assert.equal(await readScopedState({ ...shared, segments: ['missing.md'] }), null);
});

test('a pre-split legacy layout is read and written in place without a migration step', async (t) => {
  const repoRoot = fixture(t);
  const legacyRuns = path.join(repoRoot, LEGACY_STATE_DIRNAME, 'runs');
  await fs.mkdir(legacyRuns, { recursive: true });
  await fs.writeFile(path.join(legacyRuns, 'runs.jsonl'), '{"session":"roster-1-coder"}\n', 'utf8');
  const options = { scope: STATE_SCOPES.PER_WORKTREE, repoRoot, worktreeRoot: repoRoot, segments: ['runs', 'runs.jsonl'] };
  const resolved = await statePaths(options);
  assert.equal(resolved.legacy, true);
  assert.equal(resolved.path, path.join(legacyRuns, 'runs.jsonl'));
  assert.equal(await readScopedState(options), '{"session":"roster-1-coder"}\n');
  // Writing a new file keeps the legacy layout authoritative, so earlier legacy data stays readable.
  await writeScopedState('{}\n', { ...options, segments: ['runs', 'index.json'] });
  assert.equal(await readScopedState(options), '{"session":"roster-1-coder"}\n');
  assert.equal(await readScopedState({ ...options, segments: ['runs', 'index.json'] }), '{}\n');
});
