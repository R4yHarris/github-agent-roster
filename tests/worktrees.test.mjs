import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { listIssueWorktrees, assertIsolatedIssueBranches } from '../src/lib/worktrees.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { formatHelp } from '../src/shell/commands.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

test('registered issue worktrees show independent branches and actual dirty/clean state', async (t) => {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-worktree-list-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' });
  git('init', '--quiet', '-b', 'main');
  writeFileSync(path.join(root, 'README.md'), '# Root\n');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'Fixture');
  const first = path.join(root, '.worktrees', 'issue-108');
  const second = path.join(root, '.worktrees', 'issue-109');
  git('worktree', 'add', '-b', 'issue-108', first);
  git('worktree', 'add', '-b', 'issue-109', second);
  writeFileSync(path.join(first, 'README.md'), '# Dirty\n');
  const entries = await listIssueWorktrees({ cwd: root, env: process.env });
  assert.deepEqual(entries.map(({ branch, status }) => [branch, status]),
    [['issue-108', 'dirty'], ['issue-109', 'clean']]);
  assert.equal(entries[0].path.replaceAll('\\', '/').toLowerCase(), first.replaceAll('\\', '/').toLowerCase());
  assert.match(formatHelp('worktrees'), /List registered issue worktree/);
});

test('branch clashes and batch fail before launching another seat', async () => {
  assert.throws(() => assertIsolatedIssueBranches([{ branch: 'issue-108' }, { branch: 'issue-108' }]), /cannot share/);
  const shell = createDispatcher({ config, env: {}, output: { write() {} }, errorOutput: { write() {} },
    services: { repositoryBranch: () => 'main',
      runBuiltinIssue: () => assert.fail('Batch must not start a seat'),
      runBuiltinAsk: () => assert.fail('Batch must not start an Ask') } });
  await assert.rejects(shell.dispatch('/batch'), (error) =>
    error.message === 'One seat at a time. Worktrees are isolated.');
});
