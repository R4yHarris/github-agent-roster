import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { issueBranch, issueBranches, issueTask, parseIssueBranch, resolveIssueBranch } from '../src/lib/issue-branches.mjs';
import { runIssue } from '../src/lib/issue.mjs';
import { listLocalRuns, readLocalRun } from '../src/lib/local-runs.mjs';
import { readStatus } from '../src/lib/status.mjs';
import { acquireRepoLock } from '../src/lib/repo-locks.mjs';
import { assertIsolatedIssueBranches, listIssueWorktrees } from '../src/lib/worktrees.mjs';
import { classifyStrandedWork } from '../src/lib/stranded.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));
const issue = { number: 42, title: 'Update app.mjs', body: 'Update app.mjs.', state: 'OPEN',
  url: 'https://github.com/example/project/issues/42', labels: [], assignees: [] };

function fixture(t) {
  const root = mkdtempSync(path.join(process.cwd(), '.issue-branches-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (cwd, ...args) => execFileSync('git', ['-c', 'user.name=fixture',
    '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', ...args],
  { cwd, encoding: 'utf8', stdio: 'pipe' }).trim();
  git(root, 'init', '-q', '-b', 'main');
  writeFileSync(path.join(root, 'app.mjs'), 'export const value = 1;\n');
  writeFileSync(path.join(root, '.gitignore'), '.worktrees/\n');
  git(root, 'add', '.'); git(root, 'commit', '-qm', 'baseline fixture');
  git(root, 'remote', 'add', 'origin', 'https://github.com/example/project.git');
  git(root, 'update-ref', 'refs/remotes/origin/main', git(root, 'rev-parse', 'HEAD'));
  git(root, 'symbolic-ref', 'refs/remotes/origin/HEAD', 'refs/remotes/origin/main');
  const calls = [];
  const runCommand = async (program, args, cwd) => {
    calls.push({ program, args, cwd });
    if (program === 'git') return git(cwd, ...args);
    if (program === 'gh' && args[0] === 'issue') return JSON.stringify(issue);
    throw new Error('Unexpected external command');
  };
  const env = { ROSTER_MACHINE_ROOT: path.join(root, 'machine') };
  const options = { cwd: root, config, env, runCommand, recordPreparation: false,
    log() {}, start: { sync: 'offline' } };
  return { root, git, calls, env, options, worktree: path.join(root, '.worktrees', issueTask(42)) };
}

test('branch namespace parsing is exact; task and directory identifiers stay opaque and legacy-compatible', () => {
  assert.equal(issueTask(42), 'issue-42');
  assert.equal(issueBranch(42), 'agents/42');
  assert.deepEqual(issueBranches(42, 3), ['agents/42-a3', 'issue-42-a3']);
  for (const name of ['agents/42', 'issue-42']) assert.deepEqual(parseIssueBranch(name), { number: 42, attempt: null });
  for (const name of ['agents/42-a16', 'issue-42-a16']) assert.deepEqual(parseIssueBranch(name), { number: 42, attempt: 16 });
  for (const name of ['agents/042', 'agents/42-extra', 'issue-42-a17', 'user/42', 'agents/9007199254740992']) {
    assert.equal(parseIssueBranch(name), null);
  }
});

test('fresh issue creation uses agents/N from fresh trunk, stable directory/task, and reuses without moving edits', async (t) => {
  const { root, git, options, calls, worktree, env } = fixture(t);
  const first = await runIssue(42, options);
  assert.equal(first.branch, 'agents/42');
  assert.equal(first.task, 'issue-42');
  assert.equal(first.worktreePath, worktree);
  assert.equal(git(worktree, 'rev-parse', 'HEAD'), git(root, 'rev-parse', 'origin/main'));
  writeFileSync(path.join(worktree, 'app.mjs'), 'export const value = 2;\n');
  const assignment = readFileSync(first.assignmentPath, 'utf8');
  const again = await runIssue(42, options);
  assert.equal(again.reused, true);
  assert.equal(git(worktree, 'branch', '--show-current'), 'agents/42');
  assert.equal(readFileSync(first.assignmentPath, 'utf8'), assignment);
  assert.equal(readFileSync(path.join(worktree, 'app.mjs'), 'utf8'), 'export const value = 2;\n');
  assert.equal(calls.filter(({ args }) => args[0] === 'worktree' && args[1] === 'add').length, 1);
  const status = await readStatus({ issue: 42, offline: true, repoRoot: root, config, env, runCommand: options.runCommand });
  assert.equal(status.branch, 'agents/42');
  assert.equal((await readLocalRun({ number: 42, root, config, env })).task, 'issue-42');
  assert.equal((await listLocalRuns({ cwd: root, config, env }))[0].branch, 'agents/42');
  assert.equal((await listIssueWorktrees({ cwd: root, env }))[0].branch, 'agents/42');
});

test('existing legacy registered worktree and orphaned legacy branch are reused without rename or new canonical claim', async (t) => {
  for (const registered of [true, false]) {
    const { root, git, options, worktree } = fixture(t);
    if (registered) git(root, 'worktree', 'add', '-b', 'issue-42', worktree);
    else git(root, 'branch', 'issue-42');
    const result = await runIssue(42, options);
    assert.equal(result.reused, registered);
    assert.equal(result.branch, 'issue-42');
    assert.equal(git(worktree, 'branch', '--show-current'), 'issue-42');
    assert.equal(git(root, 'for-each-ref', '--format=%(refname)', 'refs/heads/agents/42'), '');
    assert.equal((await readLocalRun({ number: 42, root, config, env: options.env })).branch, 'issue-42');
  }
});

test('both namespaces refuse preparation and status before claiming or writing even when one branch is unregistered', async (t) => {
  const { root, git, options, worktree } = fixture(t);
  git(root, 'worktree', 'add', '-b', 'issue-42', worktree);
  git(root, 'branch', 'agents/42');
  let claimed = false;
  await assert.rejects(runIssue(42, { ...options, beforeWorktree: async () => { claimed = true; } }), /Ambiguous/);
  assert.equal(claimed, false);
  assert.equal(existsSync(path.join(worktree, 'ASSIGNMENT.md')), false);
  await assert.rejects(readStatus({ issue: 42, offline: true, repoRoot: root, config,
    env: options.env, runCommand: options.runCommand }), /Ambiguous/);
  assert.equal(git(worktree, 'branch', '--show-current'), 'issue-42');
});

test('foreign branches and either namespace registered at another path are never adopted or moved', async (t) => {
  for (const branch of ['human-work', 'agents/42', 'issue-42']) {
    const { root, git, options, worktree } = fixture(t);
    const target = branch === 'human-work' ? worktree : path.join(root, '.worktrees', 'other');
    git(root, 'worktree', 'add', '-b', branch, target);
    await assert.rejects(runIssue(42, options), /different branch|another worktree/);
    await assert.rejects(readStatus({ issue: 42, offline: true, repoRoot: root, config,
      env: options.env, runCommand: options.runCommand }), /different branch|another worktree/);
    assert.equal(git(target, 'branch', '--show-current'), branch);
    assert.equal(existsSync(path.join(target, 'ASSIGNMENT.md')), false);
  }
});

test('namespace aliases share the unchanged issue lock; canonical board claims and duplicate writers fail closed', async (t) => {
  const { root, options } = fixture(t);
  const lockRoot = path.join(root, 'locks');
  const canonical = parseIssueBranch('agents/42');
  const legacy = parseIssueBranch('issue-42');
  const held = await acquireRepoLock(issueTask(canonical.number), { lockRoot, holder: 'canonical', isAlive: () => true });
  try {
    await assert.rejects(acquireRepoLock(issueTask(legacy.number), { lockRoot, holder: 'legacy', isAlive: () => true }),
      (error) => error.code === 'E_LOCK_HELD');
    await assert.rejects(runIssue(42, { ...options, beforeWorktree: () =>
      acquireRepoLock(issueTask(42), { lockRoot, isAlive: () => true }) }), /held/);
    assert.equal(existsSync(path.join(root, '.worktrees', 'issue-42')), false);
  } finally { await held.release(); }
  const assigned = { ...issue, assignees: [{ login: 'worker' }] };
  assert.equal(classifyStrandedWork({ issues: [assigned], branches: ['agents/42'],
    heartbeats: [{ issue: 42, timestamp: 1000 }], now: 1000 })[0].status, 'healthy');
  assert.throws(() => classifyStrandedWork({ issues: [assigned], branches: ['agents/42', 'issue-42'], now: 1000 }), /Ambiguous/);
  assert.throws(() => assertIsolatedIssueBranches([{ branch: 'agents/42' }, { branch: 'issue-42' }]), /cannot share/);
  await resolveIssueBranch({ number: 42, repoRoot: root, runCommand: options.runCommand });
});

test('online publication lookup finds either head and refuses dual-head PR ownership', async (t) => {
  const { root, options } = fixture(t);
  await runIssue(42, options);
  for (const heads of [['agents/42'], ['issue-42'], ['agents/42', 'issue-42']]) {
    const command = async (program, args, cwd) => {
      if (program !== 'gh' || args[0] !== 'pr') return options.runCommand(program, args, cwd);
      const head = args[args.indexOf('--head') + 1];
      return JSON.stringify(heads.includes(head) ? [{ number: head === 'agents/42' ? 7 : 8,
        title: 'Implementation', url: `https://github.com/example/project/pull/${head === 'agents/42' ? 7 : 8}`,
        headRefName: head }] : []);
    };
    const request = readStatus({ issue: 42, repoRoot: root, config, env: options.env, runCommand: command });
    if (heads.length > 1) await assert.rejects(request, /Ambiguous open PR/);
    else assert.equal((await request).openPr.headRefName, heads[0]);
  }
});
