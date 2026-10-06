import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { identityHash, compareIdentity, resolveRepoIdentity, RepoIdentityError } from '../src/lib/repo-identity.mjs';


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

test('identityHash is stable and input-sensitive', () => {
  const a = identityHash({ gitCommonDir: '/repo/.git', remoteUrl: 'test-only-private-api-key' });
  const b = identityHash({ gitCommonDir: '/repo/.git', remoteUrl: 'test-only-private-api-key' });
  const c = identityHash({ gitCommonDir: '/other/.git', remoteUrl: 'test-only-private-api-key' });
  assert.equal(a, b);
  assert.notEqual(a, c);
});

test('identityHash fails closed on missing inputs', () => {
  assert.throws(() => identityHash({ gitCommonDir: '', remoteUrl: 'x' }), RepoIdentityError);
  assert.throws(() => identityHash({ gitCommonDir: '/repo/.git', remoteUrl: '  ' }), RepoIdentityError);
  assert.throws(() => identityHash(), RepoIdentityError);
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