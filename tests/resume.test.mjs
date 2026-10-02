import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { listLocalRuns, readLocalRun, recapRun } from '../src/lib/local-runs.mjs';
import { planStub, renderAssignment } from '../src/planner/stub.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { formatHelp } from '../src/shell/commands.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-resume-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' });
  git('init', '--quiet', '-b', 'main');
  writeFileSync(path.join(root, 'README.md'), '# Root\n');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'Fixture');
  const worktree = path.join(root, '.worktrees', 'issue-108');
  git('worktree', 'add', '-b', 'issue-108', worktree);
  const issue = { number: 108, title: 'Add Status', body: 'Add Status to README.md.',
    url: 'https://github.com/example/project/issues/108' };
  const plan = planStub(issue.body, { reference: 'issue:108', title: issue.title });
  writeFileSync(path.join(worktree, 'ASSIGNMENT.md'), renderAssignment(issue));
  writeFileSync(path.join(worktree, 'TASK.md'), plan.task);
  writeFileSync(path.join(worktree, 'RECIPE.yml'), plan.recipe);
  writeFileSync(path.join(worktree, 'RESULT.md'), '# Result\n\nChecks: FAIL\n- node --test exited 1\n\n## Summary\nPRIVATE_COMPLETION_BODY\n');
  writeFileSync(path.join(worktree, 'REVIEW.md'), '# Review\n\nVerdict: fail\n');
  return { root, worktree, issue, plan };
}

test('resume discovers only registered local issue runs and reconstructs their existing assignment', async (t) => {
  const { root, worktree } = fixture(t);
  const runs = await listLocalRuns({ cwd: root, config, env: {} });
  assert.equal(runs.length, 1);
  assert.equal(runs[0].issue.number, 108);
  assert.equal(runs[0].state, 'failed');
  const run = await readLocalRun({ number: 108, cwd: root, config, env: {} });
  assert.equal(run.worktreePath, worktree);
  assert.equal(run.task, 'issue-108');
  assert.equal(run.ask, 'Add Status to README.md.');
  await assert.rejects(readLocalRun({ number: 109, cwd: root, config, env: {} }), /not registered/);
});

test('recap reads only structured task, test and review facts, never completion text', async (t) => {
  const { root } = fixture(t);
  const run = await readLocalRun({ number: 108, cwd: root, config, env: {} });
  const recap = await recapRun(run, { finishReason: 'length', env: {} });
  assert.match(recap, /Outcome: Add Status \| Files: README\.md/);
  assert.match(recap, /Last test: node --test \(exit 1\) \| Review: fail \| Finish reason: length/);
  assert.doesNotMatch(recap, /PRIVATE_COMPLETION_BODY/);
  assert.equal(recap.split('\n').length, 2);
});

test('shell resume reuses the prepared worktree and its list form is documented', async () => {
  const prepared = { issue: { number: 108, title: 'Local issue' }, task: 'issue-108',
    worktreePath: 'registered-issue-108', askKind: 'slice', seat: 'coder', state: 'idle' };
  let selected;
  const shell = createDispatcher({ config, env: {}, output: { write() {} }, errorOutput: { write() {} },
    services: { repositoryBranch: () => 'main', repositoryRoot: () => process.cwd(),
      listLocalRuns: async () => [prepared], readLocalRun: async () => prepared,
      runBuiltinIssue: async (_issue, options) => { selected = options.preparedRun; return prepared; } } });
  await shell.dispatch('/resume');
  await shell.dispatch('/resume 108');
  assert.equal(selected, prepared);
  assert.match(formatHelp('resume'), /no argument/);
});

test('resuming a PLAN-only slice retains its human acceptance gate and starts no coder', async (t) => {
  const { root, worktree, plan } = fixture(t);
  unlinkSync(path.join(worktree, 'TASK.md'));
  unlinkSync(path.join(worktree, 'RECIPE.yml'));
  writeFileSync(path.join(worktree, 'PLAN.md'), plan.task.replace(/^# Task:/m, '# Plan:'));
  const prepared = await readLocalRun({ number: 108, cwd: root, config, env: {} });
  assert.equal(prepared.planMode, true);
  const shell = createDispatcher({ cwd: root, config, env: {}, output: { write() {} }, errorOutput: { write() {} },
    services: { readLocalRun: async () => prepared,
      runBuiltinIssue: () => assert.fail('Resume must not implicitly accept a PLAN-only slice') } });
  await shell.dispatch('/resume 108');
  assert.equal(shell.state.display.state, 'planning');
  assert.equal(shell.state.pendingConfirm.options.planMode, true);
});
