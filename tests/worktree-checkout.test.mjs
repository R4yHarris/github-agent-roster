import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { requireWorktreeCheckout } from '../src/lib/builtin.mjs';

function repo() {
  const root = mkdtempSync(path.join(os.tmpdir(), 'roster-worktree-check-'));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' });
  git('init', '-q', '-b', 'main');
  git('config', 'user.email', 'test@example.invalid');
  git('config', 'user.name', 'Test');
  writeFileSync(path.join(root, 'README.md'), 'x\n');
  git('add', '.');
  git('commit', '-q', '-m', 'init');
  return { root, git };
}

test('a registered issue worktree passes the checkout guard', async (t) => {
  const { root, git } = repo();
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const worktree = path.join(root, '.worktrees', 'issue-1');
  git('worktree', 'add', '-q', '-b', 'issue-1', worktree);
  await requireWorktreeCheckout(worktree);
});

test('an unregistered worktree directory fails before any seat can plan without files', async (t) => {
  const { root, git } = repo();
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const worktree = path.join(root, '.worktrees', 'issue-2');
  git('worktree', 'add', '-q', '-b', 'issue-2', worktree);
  git('worktree', 'remove', '--force', worktree);
  mkdirSync(worktree, { recursive: true });
  writeFileSync(path.join(worktree, 'ASSIGNMENT.md'), '# Ask\n');
  await assert.rejects(requireWorktreeCheckout(worktree), /not a registered Git worktree/);
});
