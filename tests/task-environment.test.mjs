import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { runIssue } from '../src/lib/issue.mjs';
import { snapshotWorktree } from '../src/runtime/excellence.mjs';

function fixture(t) {
  const root = mkdtempSync(path.join(process.cwd(), '.task-environment-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
  git('init', '--quiet', '-b', 'main');
  writeFileSync(path.join(root, 'README.md'), '# Product\n');
  writeFileSync(path.join(root, '.gitignore'), '.worktrees/\n');
  git('add', '--all');
  git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'Fixture');
  git('remote', 'add', 'origin', 'https://github.com/example/consumer.git');
  const options = { cwd: root, recordPreparation: false, env: {},
    config: { llm: {}, start: { base: 'current', sync: 'offline' }, publish: { enabled: false } },
    runCommand: async (program, args, cwd) => program === 'gh'
      ? JSON.stringify({ number: 42, title: 'Add status', body: 'Add Status to README.md.',
        state: 'OPEN', url: 'https://github.com/example/consumer/issues/42' })
      : execFileSync(program, args, { cwd, encoding: 'utf8' }),
    log: () => {} };
  return { root, git, options };
}

test('fresh consumer without an env ignore rule never gains a protected product env from task preparation', async (t) => {
  const { options, root } = fixture(t);
  const ready = await runIssue(42, options);
  assert.equal(existsSync(path.join(ready.worktreePath, '.env')), false);
  assert.ok(!ready.envPath.startsWith(ready.worktreePath + path.sep));
  assert.match(ready.envPath, /roster-environment[\\/]\.env$/);
  assert.equal(readFileSync(ready.envPath, 'utf8'), `AI_TASK=issue-42\nAI_SESSION=${ready.session}\n`);
  const files = execFileSync('git', ['ls-files', '--others', '--exclude-standard'], {
    cwd: ready.worktreePath, encoding: 'utf8',
  });
  assert.doesNotMatch(files, /\.env/);
  assert.equal((await snapshotWorktree(ready.worktreePath)).has('.env'), false);
  assert.equal(readFileSync(path.join(root, '.gitignore'), 'utf8'), '.worktrees/\n');
  assert.equal(readFileSync(path.join(ready.worktreePath, '.gitignore'), 'utf8').replace(/\r\n/g, '\n'), '.worktrees/\n');
  if (process.platform !== 'win32') assert.equal(statSync(ready.envPath).mode & 0o077, 0);
  const rerun = await runIssue(42, options);
  assert.equal(rerun.envPath, ready.envPath);
  assert.equal(rerun.reused, true);
});

test('task metadata leaves an existing consumer env untouched and visible to protection checks', async (t) => {
  const { options } = fixture(t);
  const ready = await runIssue(42, options);
  const productEnv = path.join(ready.worktreePath, '.env');
  writeFileSync(productEnv, 'USER_OWNED_SETTING=fixture-only\n');
  const before = readFileSync(productEnv);
  await runIssue(42, options);
  assert.deepEqual(readFileSync(productEnv), before);
  assert.equal((await snapshotWorktree(ready.worktreePath)).has('.env'), true);
  assert.match(execFileSync('git', ['ls-files', '--others', '--exclude-standard'], {
    cwd: ready.worktreePath, encoding: 'utf8',
  }), /\.env/);
});

test('reused task metadata refuses hardlinks and linked environment directories without writing their targets', async (t) => {
  const { root, options } = fixture(t);
  const ready = await runIssue(42, options);
  const link = path.join(root, 'linked');
  linkSync(ready.envPath, link);
  await assert.rejects(runIssue(42, options), /regular single-link/);
  rmSync(link);
  const directory = path.dirname(ready.envPath);
  rmSync(directory, { recursive: true });
  const target = path.join(root, 'other-state');
  mkdirSync(target);
  symlinkSync(target, directory, process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(runIssue(42, options), /real directories/);
  assert.equal(existsSync(path.join(target, '.env')), false);
});
