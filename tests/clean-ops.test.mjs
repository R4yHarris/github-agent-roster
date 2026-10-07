import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

import {
  CleanOpsError,
  assertSafeStatePath,
  assertStateScope,
  ensureStateDirectory,
  forbiddenStateRoots,
  removeStatePath,
  resolveRepositoryId,
  resolveStatePath,
  resolveStateRoot,
} from '../src/lib/clean-ops.mjs';
import { isContainedIn } from '../src/lib/paths.mjs';
import { identityHash } from '../src/lib/repo-identity.mjs';

/**
 * Directory junctions are reparse points that Windows allows without elevated
 * privileges, so they are the portable way to exercise reparse-point escape
 * detection in this environment.
 */
function canCreateJunction() {
  try {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'clean-ops-junction-probe-'));
    try {
      fs.mkdirSync(path.join(dir, 'target'), { recursive: true });
      fs.symlinkSync(path.join(dir, 'target'), path.join(dir, 'link'), 'junction');
      return fs.existsSync(path.join(dir, 'link'));
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  } catch {
    return false;
  }
}

const JUNCTION_SUPPORTED = canCreateJunction();

function makeRepo(t) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'clean-ops-'));
  t?.after?.(() => fsp.rm(tmp, { recursive: true, force: true }).catch(() => {}));
  const repoRoot = path.join(tmp, 'repo');
  fs.mkdirSync(path.join(repoRoot, '.git'), { recursive: true });
  // repo-identity.mjs shells out to git, so the fixture must be a real
  // checkout: init it and give it an origin remote with a non-credential
  // sentinel URL.
  const git = (args) => execFileSync('git', args, { cwd: repoRoot, encoding: 'utf8' });
  git(['init', '--quiet']);
  git(['remote', 'add', 'origin', 'https://sentinel.example/repo.git']);
  return { tmp, repoRoot };
}

function makeHandle(override = {}) {
  return {
    scope: 'worktree',
    root: path.join(os.tmpdir(), 'clean-ops-state'),
    repoId: 'repo-test',
    worktreeId: 'wt-test',
    runId: null,
    ...override,
  };
}

test('forbiddenStateRoots lists filesystem root, home and repository directories', async () => {
  const { repoRoot } = makeRepo();
  const forbidden = await forbiddenStateRoots({ repoRoot });
  const labels = forbidden.map((entry) => entry.label);

  assert.ok(labels.includes('root'), 'filesystem root must be forbidden');
  assert.ok(labels.includes('home'), 'home must be forbidden');
  assert.ok(labels.includes('repo'), 'repo must be forbidden');

  const rootEntry = forbidden.find((entry) => entry.label === 'root');
  const homeEntry = forbidden.find((entry) => entry.label === 'home');
  const repoEntry = forbidden.find((entry) => entry.label === 'repo');

  assert.equal(rootEntry.path, path.resolve(path.parse(process.cwd()).root));
  assert.equal(homeEntry.path, path.resolve(os.homedir()));
  assert.equal(repoEntry.path, path.resolve(repoRoot));
});

test('forbiddenStateRoots dedupes identical entries', async () => {
  const forbidden = await forbiddenStateRoots({ repoRoot: process.cwd(), worktreeRoot: process.cwd() });
  const seen = new Set();
  for (const entry of forbidden) {
    const key = entry.path.toLowerCase();
    assert.ok(!seen.has(key), `duplicate forbidden entry for ${entry.path}`);
    seen.add(key);
  }
});

test('resolveStateRoot delegates repository identity to repo-identity.mjs', async (t) => {
  const { repoRoot, tmp } = makeRepo(t);

  const handle = await resolveStateRoot(repoRoot, {
    scope: 'repo',
    machineRoot: { root: path.join(tmp, 'machine') },
  });

  // The state directory name is the repo-identity digest, proving the id that
  const expectedId = await resolveRepositoryId(repoRoot);
  assert.equal(handle.repoId, expectedId);
  assert.ok(handle.root.endsWith(path.join('repos', expectedId)));
  assert.equal(handle.resolvedRepoId, expectedId);
  assert.equal(handle.scope, 'repo');
});

test('resolveStateRoot refuses filesystem root, home and repository directories', async () => {
  const { repoRoot } = makeRepo();
  const root = path.parse(process.cwd()).root;
  const attempts = [root, os.homedir(), repoRoot];

  for (const attempt of attempts) {
    await assert.rejects(
      () => resolveStateRoot(repoRoot, { stateRoot: attempt }),
      (error) => {
        assert.ok(error instanceof CleanOpsError);
        assert.equal(error.code, 'CLEAN_OPS_FORBIDDEN_PATH');
        return true;
      },
      `expected refusal for ${attempt}`
    );
  }
});

test('resolveStateRoot delegates to paths.mjs and validates scope', async (t) => {
  const { repoRoot, tmp } = makeRepo(t);
  const stateRoot = path.join(tmp, 'state');
  fs.mkdirSync(stateRoot, { recursive: true });

  const handle = await resolveStateRoot(repoRoot, { stateRoot, scope: 'worktree' });
  assert.equal(handle.scope, 'worktree');
  assert.equal(handle.root, path.resolve(stateRoot));
  assert.ok(typeof handle.repoId === 'string' && handle.repoId.length > 0);
  assert.ok(handle.repoId.startsWith('sha256-') || handle.repoId.startsWith('repo-'));
  assert.equal(handle.resolvedRepoId, handle.repoId);

  await assert.rejects(
    () => resolveStateRoot(repoRoot, { stateRoot, scope: 'not-a-scope' }),
    (error) => {
      assert.ok(error instanceof CleanOpsError);
      assert.equal(error.code, 'CLEAN_OPS_SCOPE_MISMATCH');
      return true;
    }
  );
});

test('resolveRepositoryId uses src/lib/repo-identity.mjs and is stable', async (t) => {
  const { repoRoot } = makeRepo(t);
  const id = await resolveRepositoryId(repoRoot);
  assert.ok(typeof id === 'string' && id.length > 0);
  assert.equal(await resolveRepositoryId(repoRoot), id);
  await assert.rejects(() => resolveRepositoryId(''), CleanOpsError);
});

test('resolveRepositoryId provenance: repo-identity.mjs reads git metadata of repoRoot', async (t) => {
  const { repoRoot } = makeRepo(t);
  const calls = [];
  const run = async (command, args) => {
    calls.push({ command, args, cwd: undefined });
    // Deterministic sentinel answers instead of a real git checkout.
    if (args.includes('rev-parse')) {
      return { stdout: `${repoRoot}${path.sep}.git\n` };
    }
    return { stdout: 'https://sentinel.example/repo.git\n' };
  };

  const id = await resolveRepositoryId(repoRoot, { run });

  // Mirror repo-identity.mjs canonicalCommonDir: resolved, forward slashes,
  // lowercased on Windows.
  const commonDir = path.resolve(repoRoot, '.git').split(path.sep).join('/');
  const expectedCommonDir = process.platform === 'win32' ? commonDir.toLowerCase() : commonDir;
  assert.equal(id, identityHash({
    gitCommonDir: expectedCommonDir,
    remoteUrl: 'https://sentinel.example/repo.git',
  }));
  // The git child process must run against the requested repository root.
  assert.equal(calls.length, 2);
  for (const call of calls) {
    assert.equal(call.command, 'git');
    // repo-identity.mjs always passes --no-pager before the subcommand.
    assert.equal(call.args[0], '--no-pager');
  }
  // A failing git invocation fails closed: repo-identity.mjs raises its own
  // error (never a silent fallback identity), which clean-ops propagates.
  await assert.rejects(
    () => resolveRepositoryId(repoRoot, { run: () => { throw new Error('git exploded'); } }),
    (error) => error instanceof Error && /git exploded/.test(error.message)
  );
});

test('assertStateScope enforces expected scope and required fields', () => {
  const handle = makeHandle();
  assert.equal(assertStateScope(handle, 'worktree'), 'worktree');
  assert.equal(assertStateScope(handle), 'worktree');
  assert.throws(() => assertStateScope(null), CleanOpsError);
  assert.throws(() => assertStateScope({ root: '/tmp/x' }), CleanOpsError);
  assert.throws(() => assertStateScope({ scope: 'repo', root: '/tmp/x' }, 'worktree'), CleanOpsError);
});

test('resolveStatePath stays inside the state root and rejects traversal', async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'clean-ops-root-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const handle = makeHandle({ root });

  const inside = await resolveStatePath(handle, 'sub/leaf.txt');
  assert.equal(typeof inside, 'string');
  assert.ok(isContainedIn(inside, path.resolve(root)));

  for (const escape of ['..', '../..', os.homedir(), path.join(root, '..', 'elsewhere'), '/absolute/elsewhere']) {
    await assert.rejects(
      () => resolveStatePath(handle, escape),
      (error) => {
        assert.ok(error instanceof CleanOpsError);
        assert.equal(error.code, 'CLEAN_OPS_PATH_ESCAPE');
        return true;
      },
      `expected escape rejection for ${escape}`
    );
  }
});

test('resolveStatePath rejects a directory-junction reparse-point escape', { skip: !JUNCTION_SUPPORTED }, async (t) => {
  const { tmp } = makeRepo(t);
  const root = path.join(tmp, 'state');
  const outside = path.join(tmp, 'outside');
  fs.mkdirSync(path.join(root, 'link-dir'), { recursive: true });
  fs.mkdirSync(outside, { recursive: true });
  fs.writeFileSync(path.join(outside, 'secret.txt'), 'nope');
  fs.symlinkSync(outside, path.join(root, 'link-dir', 'escape'), 'junction');

  const handle = makeHandle({ root });

  await assert.rejects(
    () => resolveStatePath(handle, path.join('link-dir', 'escape', 'secret.txt')),
    (error) => {
      assert.ok(error instanceof CleanOpsError);
      assert.equal(error.code, 'CLEAN_OPS_PATH_ESCAPE');
      // The rejection must come from paths.mjs' containment walk (which lstats
      // every component and refuses reparse points), not from the local
      // realpath fallback.
      assert.equal(error.details?.via, 'paths.mjs');
      return true;
    }
  );
});

test('assertSafeStatePath refuses a reparse-point entry inside the state root', { skip: !JUNCTION_SUPPORTED }, async (t) => {
  const { tmp } = makeRepo(t);
  const root = path.join(tmp, 'state');
  const outside = path.join(tmp, 'outside');
  fs.mkdirSync(root, { recursive: true });
  fs.mkdirSync(outside, { recursive: true });
  fs.writeFileSync(path.join(outside, 'secret.txt'), 'nope');
  fs.symlinkSync(outside, path.join(root, 'alias'), 'junction');

  const handle = makeHandle({ root });

  await assert.rejects(
    () => assertSafeStatePath(handle, 'alias'),
    (error) => {
      assert.ok(error instanceof CleanOpsError);
      assert.equal(error.code, 'CLEAN_OPS_PATH_ESCAPE');
      return true;
    }
  );
});

test('assertSafeStatePath lstat-rejects a junction whose target stays inside the state root', { skip: !JUNCTION_SUPPORTED }, async (t) => {
  const { tmp } = makeRepo(t);
  const root = path.join(tmp, 'state');
  fs.mkdirSync(path.join(root, 'inside'), { recursive: true });
  // Containment alone would accept this link; only the lstat reparse-point check can refuse it.
  fs.symlinkSync(path.join(root, 'inside'), path.join(root, 'alias'), 'junction');

  await assert.rejects(
    () => assertSafeStatePath(makeHandle({ root }), 'alias'),
    (error) => {
      assert.ok(error instanceof CleanOpsError);
      assert.equal(error.code, 'CLEAN_OPS_PATH_ESCAPE');
      assert.match(error.message, /symlink or reparse point inside the state root/);
      return true;
    }
  );
});

test('assertSafeStatePath lstat-rejects a nested junction inside the state root', { skip: !JUNCTION_SUPPORTED }, async (t) => {
  const { tmp } = makeRepo(t);
  const root = path.join(tmp, 'state');
  fs.mkdirSync(path.join(root, 'runs'), { recursive: true });
  fs.mkdirSync(path.join(root, 'inside'), { recursive: true });
  fs.symlinkSync(path.join(root, 'inside'), path.join(root, 'runs', 'alias'), 'junction');

  await assert.rejects(
    () => assertSafeStatePath(makeHandle({ root }), path.join('runs', 'alias')),
    /symlink or reparse point inside the state root/
  );
});

test('removeStatePath refuses a junction escape and leaves the outside target intact', { skip: !JUNCTION_SUPPORTED }, async (t) => {
  const { tmp } = makeRepo(t);
  const root = path.join(tmp, 'state');
  const outside = path.join(tmp, 'outside');
  fs.mkdirSync(root, { recursive: true });
  fs.mkdirSync(outside, { recursive: true });
  fs.writeFileSync(path.join(outside, 'keep.txt'), 'outside data');
  fs.symlinkSync(outside, path.join(root, 'alias'), 'junction');

  await assert.rejects(
    () => removeStatePath(makeHandle({ root }), 'alias'),
    (error) => error instanceof CleanOpsError && error.code === 'CLEAN_OPS_PATH_ESCAPE'
  );
  assert.equal(fs.readFileSync(path.join(outside, 'keep.txt'), 'utf8'), 'outside data');
  assert.ok(fs.lstatSync(path.join(root, 'alias')).isSymbolicLink());
});

test('removeStatePath deletes an ordinary directory inside the state root', async (t) => {
  const { tmp } = makeRepo(t);
  const root = path.join(tmp, 'state');
  const nested = path.join(root, 'runs', 'abc');
  fs.mkdirSync(nested, { recursive: true });
  fs.writeFileSync(path.join(nested, 'data.txt'), 'x');

  const handle = makeHandle({ root });
  const result = await removeStatePath(handle, path.join('runs', 'abc'));
  assert.equal(result.removed, true);
  assert.ok(!fs.existsSync(nested));
});

test('removeStatePath refuses root, home, repo and state-root directories', async (t) => {
  const { repoRoot, tmp } = makeRepo(t);
  const root = path.join(tmp, 'state');
  fs.mkdirSync(root, { recursive: true });
  const handle = makeHandle({ root });

  const attempts = [
    ['.', 'state root itself'],
    ['..', 'parent of state root'],
    [repoRoot, 'repository root'],
    [os.homedir(), 'home directory'],
    [path.parse(process.cwd()).root, 'filesystem root'],
  ];

  for (const [target, label] of attempts) {
    await assert.rejects(
      () => removeStatePath(handle, target),
      (error) => {
        assert.ok(error instanceof CleanOpsError, `${label} must be refused`);
        assert.ok(
          error.code === 'CLEAN_OPS_FORBIDDEN_PATH' || error.code === 'CLEAN_OPS_PATH_ESCAPE',
          `${label} must be refused as unsafe`
        );
        return true;
      },
      `expected refusal for ${label}`
    );
  }
});

test('removeStatePath supports dry run without deleting', async (t) => {
  const { tmp } = makeRepo(t);
  const root = path.join(tmp, 'state');
  const nested = path.join(root, 'target');
  fs.mkdirSync(nested, { recursive: true });
  fs.writeFileSync(path.join(nested, 'data.txt'), 'x');

  const handle = makeHandle({ root });
  const result = await removeStatePath(handle, 'target', { dryRun: true });
  assert.equal(result.removed, false);
  assert.equal(result.dryRun, true);
  assert.ok(fs.existsSync(nested));
});

test('ensureStateDirectory creates the state root once and refuses protected paths', async (t) => {
  const { repoRoot, tmp } = makeRepo(t);
  const root = path.join(tmp, 'state');
  const handle = makeHandle({ root });

  const created = await ensureStateDirectory(handle);
  assert.equal(created, path.resolve(root));
  assert.ok(fs.existsSync(root));

  await assert.rejects(
    () => ensureStateDirectory(makeHandle({ root: path.resolve(repoRoot), repoRoot })),
    (error) => {
      assert.ok(error instanceof CleanOpsError);
      assert.equal(error.code, 'CLEAN_OPS_FORBIDDEN_PATH');
      return true;
    }
  );

  const worktreeRoot = path.join(tmp, 'feature-worktree');
  fs.mkdirSync(worktreeRoot, { recursive: true });
  await assert.rejects(
    () => ensureStateDirectory(makeHandle({ root: worktreeRoot, repoRoot, worktreeRoot })),
    (error) => error instanceof CleanOpsError && error.code === 'CLEAN_OPS_FORBIDDEN_PATH' && /worktree/.test(error.message)
  );
});
