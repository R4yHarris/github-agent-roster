import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { branchDrift, formatStart, resolveStart, startOptions } from '../src/lib/start.mjs';

const identity = ['-c', 'user.name=Start Test', '-c', 'user.email=start@example.invalid'];
const git = (cwd, ...args) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
const runCommand = async (program, args, cwd) => execFileSync(program, args, {
  cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, GIT_TERMINAL_PROMPT: '0' },
});

function commit(cwd, file, text) {
  writeFileSync(path.join(cwd, file), text);
  git(cwd, 'add', file);
  git(cwd, ...identity, 'commit', '-q', '-m', `edit ${file}`);
  return git(cwd, 'rev-parse', 'HEAD');
}

// origin (bare) <- upstream pushes newer main; local clone sits on a stale feature branch.
function repos(context) {
  const base = mkdtempSync(path.join(tmpdir(), 'roster-start-'));
  context.after(() => rmSync(base, { recursive: true, force: true }));
  const origin = path.join(base, 'origin.git');
  const upstream = path.join(base, 'upstream');
  const local = path.join(base, 'local');
  git(base, 'init', '-q', '--bare', '-b', 'main', origin);
  git(base, 'clone', '-q', origin, upstream);
  git(upstream, 'checkout', '-q', '-b', 'main');
  const first = commit(upstream, 'README.md', 'one\n');
  git(upstream, 'push', '-q', 'origin', 'main');
  git(base, 'clone', '-q', origin, local);
  git(local, 'checkout', '-q', '-b', 'feature/unrelated');
  const feature = commit(local, 'feature.txt', 'unrelated\n');
  const latest = commit(upstream, 'README.md', 'two\n');
  git(upstream, 'push', '-q', 'origin', 'main');
  return { base, origin, local, first, feature, latest };
}

test('new work starts from the freshly fetched trunk, not the stale feature HEAD', async (context) => {
  const { local, feature, latest } = repos(context);
  assert.notEqual(git(local, 'rev-parse', 'origin/main'), latest, 'local trunk is stale before start');
  const start = await resolveStart({ repoRoot: local, runCommand });
  assert.deepEqual({ mode: start.mode, ref: start.ref, sha: start.sha, fetched: start.fetched },
    { mode: 'trunk', ref: 'origin/main', sha: latest, fetched: true });
  const worktree = path.join(local, '.worktrees', 'issue-1');
  git(local, 'worktree', 'add', '-q', '-b', 'issue-1', worktree, start.ref);
  assert.equal(git(worktree, 'rev-parse', 'HEAD'), latest);
  assert.equal(readFileSync(path.join(worktree, 'README.md'), 'utf8').replaceAll('\r\n', '\n'), 'two\n');
  assert.notEqual(git(worktree, 'rev-parse', 'HEAD'), feature);
  assert.match(formatStart(start), new RegExp(`^Start: trunk origin/main@${latest.slice(0, 7)} \\(fetched\\)$`));
});

test('a stale origin/HEAD falls back to an existing origin/main', async (context) => {
  const { local, latest } = repos(context);
  git(local, 'symbolic-ref', 'refs/remotes/origin/HEAD', 'refs/remotes/origin/renamed');
  const start = await resolveStart({ repoRoot: local, runCommand });
  assert.deepEqual({ mode: start.mode, ref: start.ref, sha: start.sha }, { mode: 'trunk', ref: 'origin/main', sha: latest });
});

test('offline start skips fetch and reports freshness unknown instead of current', async (context) => {
  const { local, first } = repos(context);
  const start = await resolveStart({ repoRoot: local, runCommand, sync: 'offline' });
  assert.equal(start.sha, first);
  assert.equal(start.fetched, 'skipped');
  assert.match(formatStart(start), /\(freshness unknown\); fetch skipped \(offline\)/);
});

test('a failed fetch falls back to the last known trunk and says freshness is unknown', async (context) => {
  const { local, first, base } = repos(context);
  git(local, 'remote', 'set-url', 'origin', path.join(base, 'missing.git'));
  const start = await resolveStart({ repoRoot: local, runCommand });
  assert.equal(start.fetched, false);
  assert.equal(start.sha, first);
  assert.match(start.notes.join('\n'), /fetch failed .*freshness unknown/);
});

test('explicit refs resolve or fail closed, and current keeps the caller HEAD', async (context) => {
  const { local, feature } = repos(context);
  const explicit = await resolveStart({ repoRoot: local, runCommand, base: 'feature/unrelated', sync: 'offline' });
  assert.deepEqual([explicit.mode, explicit.sha], ['ref', feature]);
  await assert.rejects(resolveStart({ repoRoot: local, runCommand, base: 'no-such-branch', sync: 'offline' }),
    /Start base no-such-branch does not name a commit/);
  const current = await resolveStart({ repoRoot: local, runCommand, base: 'current' });
  assert.deepEqual([current.mode, current.ref, current.fetched], ['current', null, true]);
  await assert.rejects(resolveStart({ repoRoot: local, runCommand, base: '--upload-pack=x' }), /Git ref name/);
});

test('a repository without origin starts from current HEAD with an explicit note', async (context) => {
  const { local } = repos(context);
  git(local, 'remote', 'remove', 'origin');
  const start = await resolveStart({ repoRoot: local, runCommand });
  assert.equal(start.mode, 'current');
  assert.match(start.notes.join('\n'), /no origin remote; freshness unknown/);
  assert.match(start.notes.join('\n'), /origin default branch not found; starting from current HEAD/);
});

test('drift counts how far an existing branch is behind the fresh trunk without moving it', async (context) => {
  const { local, feature } = repos(context);
  const start = await resolveStart({ repoRoot: local, runCommand });
  assert.deepEqual(await branchDrift({ repoRoot: local, runCommand, branch: 'feature/unrelated', start }),
    { ahead: 1, behind: 1 });
  assert.equal(git(local, 'rev-parse', 'feature/unrelated'), feature);
  assert.equal(await branchDrift({ repoRoot: local, runCommand, branch: 'x', start: { ref: null } }), null);
});

test('start defaults to trunk + fetch, and config and overrides choose otherwise', () => {
  const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
  const withoutStart = example.replace(/\nstart:\n(?: {2}.*\n?)*/, '\n');
  assert.deepEqual(startOptions(parseConfig(withoutStart)), { base: 'trunk', sync: 'fetch' });
  const configured = parseConfig(`${withoutStart}\nstart:\n  base: current\n  sync: offline\n`);
  assert.deepEqual(startOptions(configured), { base: 'current', sync: 'offline' });
  assert.deepEqual(startOptions(configured, { base: 'trunk' }), { base: 'trunk', sync: 'offline' });
  assert.throws(() => parseConfig(`${withoutStart}\nstart:\n  sync: always\n`), /start.sync must be fetch or offline/);
  assert.throws(() => parseConfig(`${withoutStart}\nstart:\n  base: ..\n`), /start.base must be trunk, current/);
  assert.throws(() => startOptions({}, { sync: 'pull' }), /sync must be fetch or offline/);
});
