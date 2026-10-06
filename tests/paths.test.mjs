import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { promises as fsp } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, resolve, sep } from 'node:path';
import { test } from 'node:test';
import {
  STATE_SCOPES,
  StateHandle,
  StateRootError,
  StateScopeError,
  PathContainmentError,
  acquireLock,
  atomicMkdir,
  isContainedIn,
  newRunId,
  normalizeScope,
  platformDefaultRoots,
  releaseLock,
  resolveMachineRoot,
  resolveRepositoryIdentity,
  resolveRepositoryState,
  resolveRunPaths,
  resolveStateRoot,
  resolveWorktreeRoot,
  worktreeId,
  withLock,
} from '../src/lib/paths.mjs';
import { resolveContractsPath, resolveProjectRoot } from '../src/lib/paths.mjs';

// ---------------------------------------------------------------------------
// Path normalization: every table case is written as a POSIX-ish template and
// rewritten for the host platform, so the same table runs on win32, darwin,
// and linux. Host-dependent expectations go through `host()` helpers.
// ---------------------------------------------------------------------------

const isWindows = process.platform === 'win32';

/** Turns "C|/tmp/state" into "C:\tmp\state" on Windows, "/tmp/state" elsewhere. */
function hostPath(template) {
  if (!isWindows) return template.replace(/^C\|\//, sep);
  return template
    .replace(/^C\|\//, 'C:\\')
    .replace(/\//g, '\\');
}

function tempTemplate(t, ...parts) {
  const real = mkdtempSync(join(tmpdir(), 'roster-paths-'));
  t.after(() => rmSync(real, { recursive: true, force: true }));
  const inner = join(real, ...parts);
  mkdirSync(inner, { recursive: true });
  return { real, inner };
}

function makeRepo(t, name = 'repo') {
  const { real, inner } = tempTemplate(t, name);
  mkdirSync(join(inner, '.git'), { recursive: true });
  return { repoRoot: inner, base: real };
}

test('STATE_SCOPES use the canonical machine/repo/worktree/run vocabulary', () => {
  assert.deepEqual(STATE_SCOPES, ['machine', 'repo', 'worktree', 'run']);
  for (const scope of STATE_SCOPES) assert.equal(normalizeScope(scope), scope);
  assert.equal(normalizeScope('repository'), 'repo');
  assert.throws(() => normalizeScope('nope'), StateScopeError);
});

// ---------------------------------------------------------------------------
// Table-driven resolver cases. Each row is exercised on every platform: the
// path fixtures are normalized by hostPath() and the platform under test is
// injected explicitly instead of relying on process.platform.
// ---------------------------------------------------------------------------

for (const platform of ['win32', 'darwin', 'linux']) {
  test(`[${platform}] platform-native machine defaults`, () => {
    // Every row runs for every host platform loop: each case names the
    // platform it injects and builds its expected path with host separators,
    // so the same table is exercised on win32, darwin, and linux hosts alike.
    const cases = [
      {
        name: 'windows LOCALAPPDATA',
        platform: 'win32',
        env: { LOCALAPPDATA: hostPath('C|/Users/me/AppData/Local') },
        home: hostPath('C|/Users/me'),
        expected: hostPath('C|/Users/me/AppData/Local/roster/state'),
      },
      {
        name: 'windows falls back to home AppData/Local',
        platform: 'win32',
        env: {},
        home: hostPath('C|/Users/me'),
        expected: hostPath('C|/Users/me/AppData/Local/roster/state'),
      },
      {
        name: 'macOS Library Application Support',
        platform: 'darwin',
        env: {},
        home: isWindows ? 'D:\\Users\\me' : '/Users/me',
        expected: isWindows ? 'D:\\Users\\me\\Library\\Application Support\\roster'
          : '/Users/me/Library/Application Support/roster',
      },
      {
        name: 'linux XDG_STATE_HOME',
        platform: 'linux',
        env: { XDG_STATE_HOME: isWindows ? 'D:\\xdg\\state' : '/xdg/state' },
        home: isWindows ? 'D:\\Users\\me' : '/home/me',
        expected: isWindows ? 'D:\\xdg\\state\\roster' : '/xdg/state/roster',
      },
      {
        name: 'linux ~/.local/state fallback',
        platform: 'linux',
        env: {},
        home: isWindows ? 'D:\\Users\\me' : '/home/me',
        expected: isWindows ? 'D:\\Users\\me\\.local\\state\\roster' : '/home/me/.local/state/roster',
      },
    ];

    for (const tableCase of cases) {
      const handle = resolveMachineRoot({
        env: tableCase.env,
        platform: tableCase.platform,
        home: tableCase.home,
      });
      assert.equal(handle.root, tableCase.expected, tableCase.name);
      assert.equal(handle.scope, 'machine');
      assert.equal(handle.writable, false);
      assert.deepEqual(platformDefaultRoots({
        env: tableCase.env, platform: tableCase.platform, home: tableCase.home,
      }), { machine: tableCase.expected });
    }
  });

  test(`[${platform}] explicit PATHS_OVERRIDE wins over defaults and ROSTER_STATE_ROOT`, (t) => {
    const base = mkdtempSync(join(tmpdir(), 'roster-override-'));
    t.after(() => rmSync(base, { recursive: true, force: true }));
    const legacy = join(base, 'legacy');
    const override = join(base, 'override');
    mkdirSync(legacy);
    mkdirSync(override);

    const legacyHandle = resolveMachineRoot({ env: { ROSTER_STATE_ROOT: legacy }, platform, home: '/h' });
    assert.equal(legacyHandle.root, resolve(legacy));
    assert.equal(legacyHandle.writable, true);

    const handle = resolveMachineRoot({
      env: { PATHS_OVERRIDE: override, ROSTER_STATE_ROOT: legacy },
      platform, home: '/h',
    });
    assert.equal(handle.root, resolve(override));
    assert.equal(handle.writable, true);
    assert.equal(handle.scope, 'machine');

    assert.throws(() => resolveMachineRoot({ env: { PATHS_OVERRIDE: '' }, platform, home: '/h' }),
      StateRootError);
  });

  test(`[${platform}] repo, worktree, and run identities are stable and collision-free`, (t) => {
    const base = mkdtempSync(join(tmpdir(), 'roster-id-'));
    t.after(() => rmSync(base, { recursive: true, force: true }));
    const repoRoot = join(base, 'repo');
    mkdirSync(repoRoot);
    const env = { GIT_REMOTE_URL: 'git@github.com:acme/widget.git' };

    const identity = resolveRepositoryIdentity(repoRoot, { env });
    // Identity = filesystem-safe slug + SHA-1 digest (16 hex chars).
    assert.match(identity.repoId, /^[a-z0-9-]+-[0-9a-f]{16}$/);
    assert.ok(identity.repoId.startsWith('widget-'), 'slug derives from the remote name');
    assert.equal(identity.repoId, resolveRepositoryIdentity(repoRoot, { env }).repoId,
      'same remote => same identity regardless of checkout path');
    assert.equal(identity.repoRoot, resolve(repoRoot));

    const other = resolveRepositoryIdentity(repoRoot, {
      env: { GIT_REMOTE_URL: 'git@github.com:acme/other.git' },
    });
    assert.notEqual(other.repoId, identity.repoId, 'different remote => different identity');

    const offline = resolveRepositoryIdentity(repoRoot, { env: {} });
    assert.match(offline.repoId, /^[a-z0-9-]+-[0-9a-f]{16}$/, 'no remote still digest-stable');
    assert.equal(offline.repoId, resolveRepositoryIdentity(repoRoot, { env: {} }).repoId);
    assert.notEqual(offline.repoId, identity.repoId, 'no-remote must not collide with remote');

    const wt = worktreeId(repoRoot, join(base, 'wt-a'));
    assert.match(wt, /^wt-[0-9a-f]{16}$/);
    assert.equal(wt, worktreeId(repoRoot, join(base, 'wt-a')));
    assert.notEqual(wt, worktreeId(repoRoot, join(base, 'wt-b')));

    assert.match(newRunId({ name: 'run' }), /^run-[0-9a-z]+-[0-9a-z]+$/);
    assert.equal(newRunId({ name: 'run', timestamp: 1, random: 'x' }), 'run-1-x');
  });

  test(`[${platform}] linked worktrees, nested repos, and run paths nest correctly`, async (t) => {
    const base = mkdtempSync(join(tmpdir(), 'roster-nest-'));
    t.after(() => rmSync(base, { recursive: true, force: true }));
    const repoRoot = join(base, 'repo');
    const linked = join(base, 'linked');
    const nested = join(repoRoot, 'packages', 'nested');
    mkdirSync(nested, { recursive: true });
    mkdirSync(linked, { recursive: true });
    // The repository marker lets identity walk up from the nested checkout to
    // the same repository root, which is what makes identity path-independent.
    mkdirSync(join(repoRoot, '.git'), { recursive: true });

    const env = { PATHS_OVERRIDE: join(base, 'state') };
    mkdirSync(env.PATHS_OVERRIDE, { recursive: true });

    const state = await resolveStateRoot({ repoRoot, env, machineRoot: resolveMachineRoot({ env, platform, home: '/h' }) });
    assert.equal(state.scope, 'repo');
    assert.ok(state.root.startsWith(resolve(env.PATHS_OVERRIDE)));
    assert.match(state.root, /repos[\\/][a-z0-9-]+-[0-9a-f]{16}$/);
    assert.ok(!isContainedIn(state.root, repoRoot), 'state root must live outside the repo');
    assert.ok(!isContainedIn(repoRoot, state.root), 'repo must not contain the state root');

    // Same identity from the nested checkout: identity is not the absolute path.
    const nestedState = await resolveStateRoot({
      repoRoot: nested, env, machineRoot: resolveMachineRoot({ env, platform, home: '/h' }),
    });
    assert.equal(nestedState.root, state.root);

    const worktree = await resolveWorktreeRoot({
      repoRoot, worktreeRoot: linked, env, machineRoot: resolveMachineRoot({ env, platform, home: '/h' }),
    });
    assert.equal(worktree.scope, 'worktree');
    assert.ok(worktree.root.startsWith(state.root));

    const run = await resolveRunPaths({
      repoRoot, worktreeRoot: linked, runIdentifier: 'abc', env,
      machineRoot: resolveMachineRoot({ env, platform, home: '/h' }),
    });
    assert.equal(run.runId, 'abc');
    assert.equal(run.paths.log, join(run.runRoot, 'run.log'));
    assert.equal(run.paths.lock, join(run.runRoot, 'run.lock'));
    assert.equal(run.handle.scope, 'run');
    assert.equal(run.handle.repoId, worktree.repoId);
    assert.equal(run.stateRoot, worktree.root);

    await assert.rejects(() => resolveRunPaths({
      repoRoot, worktreeRoot: linked, runIdentifier: '../escape', env,
      machineRoot: resolveMachineRoot({ env, platform, home: '/h' }),
    }), StateRootError);
  });

  test(`[${platform}] scope mismatch and containment are typed, never silent`, async (t) => {
    const base = mkdtempSync(join(tmpdir(), 'roster-scope-'));
    t.after(() => rmSync(base, { recursive: true, force: true }));
    const repoRoot = join(base, 'repo');
    mkdirSync(repoRoot);
    const machineRoot = resolveMachineRoot({ env: {}, platform, home: '/h' });

    assert.throws(() => machineRoot.repositoryPath('x'), StateScopeError);
    assert.throws(() => machineRoot.scopedPath('repo', 'x'), StateScopeError);
    const scopeError = (() => {
      try {
        machineRoot.worktreePath('x');
        return null;
      } catch (error) {
        return error;
      }
    })();
    assert.equal(scopeError.code, 'E_SCOPE');
    assert.match(scopeError.message, /machine.*worktree|worktree.*machine/);

    const machineTemplate = hostPath('C|/machine-state');
    assert.equal(isContainedIn(join(machineTemplate, 'a'), machineTemplate), true);
    // Two sibling roots under one parent are disjoint: neither contains the
    // other, so a state root beside a repository can never shadow it.
    assert.equal(isContainedIn(hostPath('C|/other'), machineTemplate), false);
    assert.equal(isContainedIn(hostPath('C|/machine-state-x'), machineTemplate), false);

    // Windows drive/case behavior: an injected win32 platform compares segments
    // case-insensitively, while POSIX stays case-sensitive.
    const parent = hostPath('C|/Machine-State');
    const childSameCase = join(parent, 'Repos');
    const childOtherCase = hostPath('C|/machine-state/repos');
    assert.equal(isContainedIn(childSameCase, parent, { platform: 'win32' }), true);
    assert.equal(isContainedIn(childOtherCase, parent, { platform: 'win32' }), true,
      'win32 containment folds case');
    assert.equal(isContainedIn(childOtherCase, parent, { platform: 'linux' }), false,
      'POSIX containment keeps case');
    assert.equal(isContainedIn(hostPath('C|/other'), parent, { platform: 'win32' }), false);

    await assert.rejects(() => resolveStateRoot({
      repoRoot,
      env: {},
      machineRoot: { root: repoRoot, platform, home: '/h' },
    }), PathContainmentError);

    const handle = new StateHandle({ scope: 'run', root: base, platform, home: '/h', writable: true });
    assert.equal(handle.path('logs', 'x.txt'), join(base, 'logs', 'x.txt'));
    assert.throws(() => handle.path('..', 'escape'), PathContainmentError);
  });
}

// ---------------------------------------------------------------------------
// Host-real symlink/reparse refusal: every table row above is platform-injected,
// but this one exercises the real filesystem, including Windows reparse points.
// ---------------------------------------------------------------------------

test('symlinked path components are refused on every platform that has them', (t) => {
  const { repoRoot, base } = makeRepo(t);
  const link = join(base, 'link');
  let linked = false;
  try {
    // On Windows the first choice is a directory junction, which does not need
    // the symlink privilege; elsewhere (and if that fails) a plain dir symlink.
    symlinkSync(repoRoot, link, isWindows ? 'junction' : 'dir');
    linked = true;
  } catch (error) {
    try {
      symlinkSync(repoRoot, link, 'dir');
      linked = true;
    } catch (fallback) {
      t.skip(`symlinks/reparse points unavailable on ${process.platform}: ` +
        `${fallback.code ?? fallback.message}`);
    }
  }
  if (linked) {
    assert.throws(() => resolveMachineRoot({ env: { PATHS_OVERRIDE: link } }), StateRootError);
  }
});

// ---------------------------------------------------------------------------
// atomicMkdir + lock ownership, including crash recovery.
// ---------------------------------------------------------------------------

test('atomicMkdir is idempotent and creates mode 0o700 directories', async (t) => {
  const { inner } = tempTemplate(t, 'mkdir');
  const target = join(inner, 'new', 'deep');
  // A loosened baseline on the same host: if 0o700 ever regressed to 0o755
  // (the umask default), the difference must show up here, not in a check that
  // only re-reads what the implementation just wrote.
  const baseline = join(inner, 'baseline');
  await fsp.mkdir(baseline, { mode: 0o755 });
  if (process.platform !== 'win32') {
    assert.equal(statSync(baseline).mode & 0o777, 0o755,
      'baseline proves this host reports POSIX mode bits at all');
  }
  assert.equal(await atomicMkdir(target), target);
  assert.equal(await atomicMkdir(target), target, 'second call is a no-op');
  if (process.platform !== 'win32') {
    // Windows does not track POSIX permission bits, so the assertion is only
    // meaningful where the OS reports them.
    assert.equal(statSync(target).mode & 0o777, 0o700, 'directory is created 0o700');
    assert.notEqual(statSync(target).mode & 0o777, statSync(baseline).mode & 0o777,
      '0o700 is the implementation choice, not the ambient default');
  }
});

test('lock shape is {pid, runId, acquiredAt} and release is owner-gated', async (t) => {
  const { inner } = tempTemplate(t, 'lock');
  const lockPath = join(inner, 'run.lock');

  const lock = await acquireLock(lockPath, { runId: 'abc' });
  assert.equal(typeof lock.pid, 'number');
  assert.equal(lock.runId, 'abc');
  assert.ok(lock.acquiredAt);
  assert.equal(lock.path, lockPath);

  assert.equal(await releaseLock(lockPath, { pid: -1, runId: 'abc' }), false,
    'a foreign pid may not release someone else’s lock');
  assert.equal(await releaseLock(lockPath, { pid: lock.pid, runId: 'abc' }), true);
  assert.equal(await releaseLock(lockPath, { pid: lock.pid, runId: 'abc' }), false,
    'releasing twice is a no-op');
});

test('live locks block and dead owners are stolen (crash recovery)', async (t) => {
  const { inner } = tempTemplate(t, 'crash');
  const lockPath = join(inner, 'run.lock');
  mkdirSync(inner, { recursive: true });

  const livePid = process.pid;
  writeFileSync(lockPath, `${JSON.stringify({ pid: livePid, runId: 'live', acquiredAt: new Date().toISOString() })}\n`);
  await assert.rejects(() => acquireLock(lockPath, { runId: 'second' }), StateRootError);

  writeFileSync(lockPath, `${JSON.stringify({ pid: 999999, runId: 'dead', acquiredAt: new Date().toISOString() })}\n`);
  const stolen = await acquireLock(lockPath, { runId: 'stolen' });
  assert.equal(stolen.runId, 'stolen');
  await stolen.release();

  const inside = await withLock(lockPath, async (held) => {
    assert.equal(held.runId, null);
    return 'done';
  }, { runId: null });
  assert.equal(inside, 'done');
  assert.equal(await releaseLock(lockPath, { pid: process.pid }), false);
});

test('locks without a valid owner pid are never stolen without a dead-owner proof', async (t) => {
  const { inner } = tempTemplate(t, 'malformed');
  mkdirSync(inner, { recursive: true });
  const lockPath = join(inner, 'run.lock');

  // Malformed JSON and an empty file both leave the owner unknown: no liveness
  // probe can prove the owner dead, so the lock must be refused, not removed.
  for (const contents of ['not json', '', '{"runId":"x"}', '{"pid":"nope"}']) {
    writeFileSync(lockPath, contents);
    await assert.rejects(() => acquireLock(lockPath, { runId: 'second' }), StateRootError,
      `contents ${JSON.stringify(contents)} must not be stolen`);
    assert.equal(statSync(lockPath, { throwIfNoEntry: false })?.isFile(), true,
      'the unattributable lock file is left in place');
  }
});

test('the dead-owner liveness probe is the only mechanism that removes a held lock', async (t) => {
  const { inner } = tempTemplate(t, 'probe-order');
  mkdirSync(inner, { recursive: true });
  const lockPath = join(inner, 'run.lock');
  writeFileSync(lockPath, `${JSON.stringify({ pid: 999999, runId: 'gone', acquiredAt: new Date().toISOString() })}\n`);

  const seen = [];
  const isAlive = (pid) => {
    seen.push(pid);
    return false; // probe: the recorded owner is provably dead
  };
  let removed = false;
  // The seam must preserve the production unlink's effect (removing the file)
  // while recording that it fired: the resolver re-creates the lock right after
  // the steal, so an unlink that leaves the file in place would spin forever.
  const unlink = async (file) => {
    removed = true;
    await fsp.unlink(file);
  };

  const stolen = await acquireLock(lockPath, { runId: 'stolen', isAlive, unlink });
  assert.deepEqual(seen, [999999], 'the probe ran on the recorded owner pid before stealing');
  assert.equal(removed, true, 'the stale lock was removed only after the probe proved the pid dead');
  assert.equal(stolen.runId, 'stolen');
  assert.equal(statSync(lockPath).isFile(), true, 'a fresh lock file now exists');
  const fresh = JSON.parse(await fsp.readFile(lockPath, 'utf8'));
  assert.equal(fresh.runId, 'stolen', 'the stale lock was replaced, never silently reused');

  // A live owner is never probed into a steal: the lock stays untouched. The
  // stale-owner deadline is in the past, so the loop must raise immediately
  // instead of sleeping on its poll interval.
  writeFileSync(lockPath, `${JSON.stringify({ pid: process.pid, runId: 'live', acquiredAt: new Date().toISOString() })}\n`);
  const liveSeen = [];
  const startedAt = Date.now();
  await assert.rejects(
    () => acquireLock(lockPath, {
      runId: 'second',
      isAlive: () => { liveSeen.push(1); return true; },
      waitMs: 0,
      pollMs: 10_000,
    }),
    StateRootError,
  );
  assert.equal(Date.now() - startedAt < 10_000, true,
    'a live owner is refused as soon as the deadline has passed, without another poll sleep');
  assert.deepEqual(liveSeen, [1], 'the live owner was probed and found alive');
  assert.equal(JSON.parse(await fsp.readFile(lockPath, 'utf8')).runId, 'live',
    'the live lock is left exactly as the owner wrote it');
});

test('preserved exports keep working: resolveProjectRoot and resolveContractsPath', (t) => {
  const { repoRoot } = makeRepo(t);
  assert.equal(resolveProjectRoot(repoRoot), resolve(repoRoot));
  assert.throws(() => resolveContractsPath({ repoRoot, cwd: repoRoot, env: {} }), /github-agent-contracts is required/);
});
