import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, mkdirSync, renameSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { identityHash, compareIdentity, resolveRepoIdentity, RepoIdentityError } from '../src/lib/repo-identity.mjs';
import { resolveRepositoryState, assertNoSymlinkComponents, ContainmentError } from '../src/lib/paths.mjs';


test('a repository and its linked worktree resolve to the same identity', async (context) => {
  const temp = mkdtempSync(path.join(tmpdir(), 'roster-identity-'));
  context.after(() => rmSync(temp, { recursive: true, force: true }));
  const repo = path.join(temp, 'repo');
  const git = (...args) => execFileSync('git', args, { cwd: repo, stdio: 'pipe' });
  execFileSync('git', ['init', '-q', repo], { stdio: 'pipe' });
  git('remote', 'add', 'origin', 'https://example.invalid/test-only-private-api-key.git');
  git('-c', 'user.name=t', '-c', 'user.email=t@example.com', 'commit', '-q', '--allow-empty', '-m', 'init');
  git('worktree', 'add', '-q', '--detach', path.join(temp, 'linked'));
  const main = await resolveRepoIdentity({ repoRoot: repo });
  assert.equal(await resolveRepoIdentity({ repoRoot: path.join(temp, 'linked') }), main);
  assert.equal(main.includes('test-only-private-api-key'), false);
});

test('identityHash never stores the remote url in plaintext', () => {
  const sentinel = 'test-only-private-api-key';
  const identity = identityHash({ gitCommonDir: '/repo/.git', remoteUrl: sentinel });
  assert.match(identity, /^sha256-[0-9a-f]{64}$/);
  assert.equal(identity.includes(sentinel), false);
});

test('identityHash is stable, clone-path invariant, and remote-sensitive', () => {
  const a = identityHash({ gitCommonDir: '/repo/.git', remoteUrl: 'test-only-private-api-key' });
  const b = identityHash({ gitCommonDir: '/elsewhere/clone/.git', remoteUrl: 'test-only-private-api-key' });
  const c = identityHash({ gitCommonDir: '/repo/.git', remoteUrl: 'test-only-other-sentinel' });
  // Same remote, different checkout/common dir: identical identity (the
  // point of this task: re-clones and moved checkouts never split identity).
  assert.equal(a, b);
  // Different remote: distinct identity.
  assert.notEqual(a, c);
});

test('identityHash fails closed on missing inputs', () => {
  assert.throws(() => identityHash({ gitCommonDir: '/repo/.git', remoteUrl: '' }), RepoIdentityError);
  assert.throws(() => identityHash({ gitCommonDir: '/repo/.git', remoteUrl: '  ' }), RepoIdentityError);
  assert.throws(() => identityHash(), RepoIdentityError);
});

test('network identity normalizes transport and credentials without merging distinct servers', () => {
  const hash = (remoteUrl) => identityHash({ remoteUrl });
  const expected = hash('https://git.example.invalid/team/repo.git');
  for (const remote of [
    'git@git.example.invalid:team/repo.git',
    'ssh://git@git.example.invalid:22/team/repo.git',
    'https://reader@git.example.invalid:443/team/repo.git/',
    'https://GIT.EXAMPLE.INVALID/team/repo',
  ]) assert.equal(hash(remote), expected, remote);
  assert.notEqual(hash('https://git.example.invalid:8443/team/repo.git'),
    hash('https://git.example.invalid:9443/team/repo.git'));
  assert.notEqual(hash('https://git.example.invalid/Team/Repo.git'), expected);
  assert.notEqual(hash('https://git.example.invalid/other/repo.git'), expected);
  assert.equal(hash('git@github.com:Owner/Repo.git'), hash('https://github.com/owner/repo'));
  assert.notEqual(hash('/srv/Team/repo.git'), hash('/srv/team/repo.git'));
  assert.throws(() => hash('https://git.example.invalid/team/repo?selector=other'), RepoIdentityError);
});

test('relative local origins resolve against the checkout rather than process cwd', async () => {
  const run = async (_cmd, args) => ({
    stdout: args.includes('--git-common-dir') ? '.git\n' : '../origin\n',
  });
  const a = await resolveRepoIdentity({ repoRoot: path.resolve('group-a', 'clone'), run });
  const b = await resolveRepoIdentity({ repoRoot: path.resolve('group-b', 'clone'), run });
  assert.notEqual(a, b);
  assert.equal(a, identityHash({ remoteUrl: path.resolve('group-a', 'origin') }));
});

test('resolveRepoIdentity derives identity from git metadata', async () => {
  const calls = [];
  const run = async (cmd, args) => {
    calls.push([cmd, ...args]);
    if (args.includes('--git-common-dir')) return { stdout: '/repo/.git\n' };
    return { stdout: 'test-only-private-api-key\n' };
  };
  const identity = await resolveRepoIdentity({ repoRoot: '/repo', run });
  assert.match(identity, /^sha256-[0-9a-f]{64}$/);
  assert.equal(identity.includes('test-only-private-api-key'), false);
  assert.deepEqual(calls[0], ['git', '--no-pager', 'rev-parse', '--git-common-dir']);
  assert.deepEqual(calls[1], ['git', '--no-pager', 'config', '--get', 'remote.origin.url']);
});

test('resolveRepoIdentity fails closed when git metadata is unreadable', async () => {
  const run = async () => { throw new Error('fatal: not a git repository'); };
  await assert.rejects(
    () => resolveRepoIdentity({ repoRoot: '/repo', run }),
    (error) => error instanceof RepoIdentityError && error.code === 'E_GIT_METADATA' &&
      /could not read the git/.test(error.message),
  );
});

test('resolveRepoIdentity fails closed when the origin remote is unconfigured', async () => {
  const run = async (cmd, args) => {
    if (args.includes('--git-common-dir')) return { stdout: '/repo/.git\n' };
    throw new Error("error: key does not contain a section: remote.origin.url");
  };
  await assert.rejects(
    () => resolveRepoIdentity({ repoRoot: '/repo', run }),
    (error) => error instanceof RepoIdentityError && error.code === 'E_GIT_METADATA' &&
      /git remote add origin/.test(error.message),
  );
});

test('a re-clone of the same repository at a different path resolves to the same identity', async (context) => {
  const temp = mkdtempSync(path.join(tmpdir(), 'roster-reclone-'));
  context.after(() => rmSync(temp, { recursive: true, force: true }));
  const first = path.join(temp, 'first-clone');
  const second = path.join(temp, 'second-clone');
  const sentinel = 'test-only-private-api-key';
  const runGit = (repo, ...args) => execFileSync('git', args, { cwd: repo, stdio: 'pipe' });
  const seed = path.join(temp, 'origin');
  execFileSync('git', ['init', '-q', seed], { stdio: 'pipe' });
  runGit(seed, 'remote', 'add', 'origin', `https://example.invalid/${sentinel}.git`);
  runGit(seed, '-c', 'user.name=t', '-c', 'user.email=t@example.com', 'commit', '-q', '--allow-empty', '-m', 'seed');
  for (const clone of [first, second]) {
    execFileSync('git', ['clone', '-q', '--no-local', seed, clone], { stdio: 'pipe' });
  }
  const identityFirst = await resolveRepoIdentity({ repoRoot: first });
  const identitySecond = await resolveRepoIdentity({ repoRoot: second });
  assert.equal(identitySecond, identityFirst);
  assert.match(identityFirst, /^sha256-[0-9a-f]{64}$/);
  assert.equal(identityFirst.includes(sentinel), false);
  assert.equal(compareIdentity(identityFirst, identitySecond).status, 'match');
});

test('moving a checkout to a new directory keeps the same identity', async (context) => {
  const temp = mkdtempSync(path.join(tmpdir(), 'roster-move-'));
  context.after(() => rmSync(temp, { recursive: true, force: true }));
  const origin = path.join(temp, 'origin');
  const moved = path.join(temp, 'moved', 'checkout');
  const sentinel = 'test-only-private-api-key';
  const runGit = (repo, ...args) => execFileSync('git', args, { cwd: repo, stdio: 'pipe' });
  execFileSync('git', ['init', '-q', origin], { stdio: 'pipe' });
  runGit(origin, 'remote', 'add', 'origin', `https://example.invalid/${sentinel}.git`);
  runGit(origin, '-c', 'user.name=t', '-c', 'user.email=t@example.com', 'commit', '-q', '--allow-empty', '-m', 'seed');
  const before = await resolveRepoIdentity({ repoRoot: origin });
  mkdirSync(path.join(temp, 'moved'), { recursive: true });
  renameSync(origin, moved);
  const after = await resolveRepoIdentity({ repoRoot: moved });
  assert.equal(after, before);
  assert.equal(compareIdentity(before, after).status, 'match');
});

test('repositories with different remotes never share an identity', async (context) => {
  const temp = mkdtempSync(path.join(tmpdir(), 'roster-distinct-'));
  context.after(() => rmSync(temp, { recursive: true, force: true }));
  const runGit = (repo, ...args) => execFileSync('git', args, { cwd: repo, stdio: 'pipe' });
  for (const [name, remote] of [['a', 'https://example.invalid/a/test-only-private-api-key.git'],
    ['b', 'https://other.invalid/b/test-only-other-sentinel.git']]) {
    const repo = path.join(temp, name);
    execFileSync('git', ['init', '-q', repo], { stdio: 'pipe' });
    runGit(repo, 'remote', 'add', 'origin', remote);
  }
  const identityA = await resolveRepoIdentity({ repoRoot: path.join(temp, 'a') });
  const identityB = await resolveRepoIdentity({ repoRoot: path.join(temp, 'b') });
  assert.notEqual(identityA, identityB);
  assert.equal(compareIdentity(identityA, identityB).status, 'mismatch');
});

test('repository state roots stay distinct for different repositories and reject symlinked roots', async (context) => {
  const temp = mkdtempSync(path.join(tmpdir(), 'roster-state-'));
  context.after(() => rmSync(temp, { recursive: true, force: true }));
  const runGit = (repo, ...args) => execFileSync('git', args, { cwd: repo, stdio: 'pipe' });
  const a = path.join(temp, 'repo-a');
  const b = path.join(temp, 'repo-b');
  for (const [repo, sentinel] of [[a, 'test-only-private-api-key'], [b, 'test-only-other-sentinel']]) {
    execFileSync('git', ['init', '-q', repo], { stdio: 'pipe' });
    runGit(repo, 'remote', 'add', 'origin', `https://example.invalid/${sentinel}.git`);
  }
  const machineRoot = path.join(temp, 'state');
  mkdirSync(machineRoot);
  const env = { GIT_REMOTE_URL: '' };
  const stateA = resolveRepositoryState({ repoRoot: a, env, machineRoot: { root: machineRoot, writable: true } });
  const stateB = resolveRepositoryState({ repoRoot: b, env, machineRoot: { root: machineRoot, writable: true } });
  assert.notEqual(stateA.root, stateB.root);
  assert.equal(isContained(stateA.root, stateB.root), false);
  assert.equal(isContained(stateB.root, stateA.root), false);
  // Symlinked state roots are still rejected.
  const realState = path.join(temp, 'real-state');
  mkdirSync(realState);
  const linked = path.join(temp, 'linked-state');
  try {
    symlinkSync(realState, linked, 'dir');
  } catch (error) {
    if (error.code === 'EACCES' || error.code === 'EPERM' || error.code === 'EINVAL' || error.code === 'UNKNOWN') {
      symlinkSync(realState, linked, 'junction');
    }
  }
  assert.throws(
    () => assertNoSymlinkComponents(linked, { scope: 'machine' }),
    (error) => error instanceof ContainmentError,
  );
  function isContained(child, parent) {
    const relative = path.relative(parent, child);
    return relative !== '' && !relative.startsWith('..') && !path.isAbsolute(relative);
  }
});

test('compareIdentity: match, initialize, and mismatch paths', () => {
  const identity = identityHash({ gitCommonDir: '/repo/.git', remoteUrl: 'test-only-private-api-key' });
  assert.deepEqual(compareIdentity(identity, identity), { status: 'match' });
  assert.deepEqual(compareIdentity(null, identity), { status: 'initialize', identity });
  assert.deepEqual(compareIdentity(undefined, identity), { status: 'initialize', identity });
  const mismatch = compareIdentity('sha256-' + '0'.repeat(64), identity);
  assert.equal(mismatch.status, 'mismatch');
  assert.equal(mismatch.recorded, 'sha256-' + '0'.repeat(64));
  assert.equal(mismatch.current, identity);
  assert.match(mismatch.message, /does not match/);
});