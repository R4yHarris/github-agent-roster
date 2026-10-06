import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { passingReview } from './helpers/review.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { runOnlyReview } from '../src/lib/review.mjs';
import { requirePassingReview } from '../src/seats/reviewer.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { formatHelp } from '../src/shell/commands.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: review-model'));

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-review-command-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' });
  git('init', '--quiet', '-b', 'issue-108');
  writeFileSync(path.join(root, 'README.md'), '# Before\n');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'Fixture');
  writeFileSync(path.join(root, 'README.md'), '# After\n');
  const task = planStub('Update README.md.').task;
  writeFileSync(path.join(root, 'TASK.md'), task);
  const resultPath = path.join(root, 'RESULT.md');
  writeFileSync(resultPath, '# Result\n\nChecks: PASS\n- node --test exited 0\n');
  return { repoRoot: root, worktreePath: root, issue: { number: 108 }, task: 'issue-108', askKind: 'slice',
    result: { mode: 'llm', model: 'review-model', resultPath, turns: 1, summary: 'Product updated.',
      tests: { exit_code: 0 }, excellence: { pass: true, files: ['README.md'] } } };
}

test('explicit reviewer receives no tools and product writes are refused', async (t) => {
  const run = fixture(t);
  let calls = 0;
  const reviewed = await runOnlyReview(run, { repoRoot: run.repoRoot, config, env: {}, errorOutput: { write() {} },
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.tools, undefined);
      assert.match(body.messages[1].content, /# After/);
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', content: null,
        tool_calls: [{ id: 'write', type: 'function', function: { name: 'write_file',
          arguments: '{"path":"README.md","content":"forged"}' } }] } }] });
    } });
  assert.equal(calls, 1);
  assert.equal(reviewed.review.verdict, 'fail');
  assert.equal(readFileSync(path.join(run.worktreePath, 'README.md'), 'utf8'), '# After\n');
  await assert.rejects(requirePassingReview(reviewed), /passing REVIEW/);
  await assert.rejects(runOnlyReview(reviewed, { repoRoot: run.repoRoot, config, env: {} }), /use \/review --again/);
  const second = await runOnlyReview(reviewed, { repoRoot: run.repoRoot, config, env: {}, again: true,
    errorOutput: { write() {} }, fetchImpl: async (_url, request) => Response.json({ choices: [{ finish_reason: 'stop',
      message: { role: 'assistant', content: passingReview(JSON.parse(request.body), { reasons: ['Current diff meets checks.'],
        security_notes: ['No protected products changed.'] }) } }] }) });
  assert.equal(second.review.verdict, 'pass');
  await requirePassingReview(second);
  assert.match(formatHelp('review'), /--again/);
});

test('failed explicit review blocks shell publication until the existing bypass is explicit', async () => {
  let prepared = 0;
  const run = { issue: { number: 108 }, task: 'issue-108', worktreePath: 'issue-108',
    askKind: 'slice', result: { summary: 'Existing coder evidence.' } };
  const shell = createDispatcher({ config, env: { AI_MODEL: 'review-model' }, output: { write() {} },
    errorOutput: { write() {} }, services: { repositoryBranch: () => 'main',
      runOnlyReview: async () => ({ ...run, review: { verdict: 'fail' } }),
      resolveContractsPath: () => 'contracts', publicationTask: () => 'issue-108',
      prepareBuiltinPublication: () => { prepared += 1; } } });
  shell.state.lastRun = run;
  await shell.dispatch('/review');
  await assert.rejects(shell.dispatch('/publish'), /passing REVIEW/);
  assert.equal(prepared, 0);
  await shell.dispatch('/publish --skip-review');
});
