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
import { parseTaskDocument } from '../src/planner/task.mjs';

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
  assert.equal(reviewed.delivery.reviewer.attempt, 1);
  assert.equal(readFileSync(path.join(run.worktreePath, 'README.md'), 'utf8'), '# After\n');
  await assert.rejects(requirePassingReview(reviewed), /passing REVIEW/);
  await assert.rejects(runOnlyReview(reviewed, { repoRoot: run.repoRoot, config, env: {} }), /use \/review --again/);
  const second = await runOnlyReview(reviewed, { repoRoot: run.repoRoot, config, env: {}, again: true,
    errorOutput: { write() {} }, fetchImpl: async (_url, request) => Response.json({ choices: [{ finish_reason: 'stop',
      message: { role: 'assistant', content: passingReview(JSON.parse(request.body), { reasons: ['Current diff meets checks.'],
        security_notes: ['No protected products changed.'] }) } }] }) });
  assert.equal(second.review.verdict, 'pass');
  assert.equal(second.delivery.reviewer.id, reviewed.delivery.reviewer.id);
  assert.equal(second.delivery.reviewer.attempt, 2);
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

test('review-only retries execute fresh approved observations and retain dependency publication binding', async (t) => {
  const run = fixture(t);
  mkdirSync(path.join(run.worktreePath, 'src'));
  writeFileSync(path.join(run.worktreePath, 'src', 'dependency.mjs'), 'export const value = 4;\n');
  const git = (...args) => execFileSync('git', args, { cwd: run.worktreePath, encoding: 'utf8' });
  git('add', '--', 'src/dependency.mjs');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'Unchanged dependency');
  const { acceptance_checks } = parseTaskDocument(readFileSync(path.join(run.worktreePath, 'TASK.md'), 'utf8'));
  const independentVerification = { docker: {
    enabled: true, image: `sha256:${'a'.repeat(64)}`, additionalFiles: ['src/dependency.mjs'],
    verifiers: [{ name: 'oracle.mjs', source: "console.log('approved');\n" }],
    plan: acceptance_checks.map((_, index) => ({
      check_id: index + 1, script: 'verifier:oracle.mjs', args: [],
      expected_stdout: 'approved\n', expected_stderr: '', expected_exit: 0,
      evidence: 'Host-approved fixture observation over exact reviewed README bytes',
    })),
  } };
  const observed = [];
  const options = { repoRoot: run.repoRoot, config, env: {}, errorOutput: { write() {} },
    independentVerification,
    runVerificationCommand: async (_command, args) => {
      if (args[0] === 'rm') return {};
      if (args.includes('-e')) return { stdout: 'v20.20.2\n', stderr: '' };
      const mount = args[args.indexOf('--mount') + 1];
      const snapshot = mount.slice('type=bind,source='.length, mount.indexOf(',target='));
      assert.equal(readFileSync(path.join(snapshot, 'src', 'dependency.mjs'), 'utf8'), 'export const value = 4;\n');
      const product = readFileSync(path.join(snapshot, 'README.md'), 'utf8');
      observed.push(product);
      return { stdout: product === '# Approved\n' ? 'approved\n' : 'contradiction\n', stderr: '' };
    },
    fetchImpl: async (_url, request) => Response.json({ choices: [{ finish_reason: 'stop',
      message: { role: 'assistant', content: passingReview(JSON.parse(request.body)) } }] }),
  };
  const rejected = await runOnlyReview(run, options);
  assert.equal(rejected.review.verdict, 'fail');
  assert.equal(rejected.review.independentVerification.status, 'fail');
  await assert.rejects(requirePassingReview(rejected), /passing REVIEW/);
  const originalReview = readFileSync(path.join(run.worktreePath, 'REVIEW.md'), 'utf8');
  await assert.rejects(runOnlyReview(rejected, {
    ...options, independentVerification: undefined, again: true,
    fetchImpl: async () => assert.fail('Missing host authority must not invoke a model'),
    runVerificationCommand: async () => assert.fail('Missing host authority must not execute'),
  }), /explicitly approved again/);
  assert.equal(readFileSync(path.join(run.worktreePath, 'REVIEW.md'), 'utf8'), originalReview);
  writeFileSync(path.join(run.worktreePath, 'README.md'), '# Approved\n');
  const accepted = await runOnlyReview({ ...rejected, result: run.result }, { ...options, again: true });
  assert.equal(accepted.review.verdict, 'pass', accepted.review.content);
  assert.equal(observed.length, acceptance_checks.length * 2);
  assert.deepEqual(observed.slice(0, acceptance_checks.length), acceptance_checks.map(() => '# After\n'));
  assert.deepEqual(observed.slice(acceptance_checks.length), acceptance_checks.map(() => '# Approved\n'));
  await requirePassingReview(accepted);
  writeFileSync(path.join(run.worktreePath, 'src', 'dependency.mjs'), 'export const value = 3;\n');
  await assert.rejects(requirePassingReview(accepted), /changed after review/);
});

test('malformed explicit review-only approval fails closed without model or backend execution', async (t) => {
  const run = fixture(t);
  const reviewed = await runOnlyReview(run, {
    repoRoot: run.repoRoot, config, env: {}, errorOutput: { write() {} },
    independentVerification: { docker: { enabled: true } },
    fetchImpl: async () => assert.fail('Invalid approval must not invoke a model'),
    runVerificationCommand: async () => assert.fail('Invalid approval must not execute'),
  });
  assert.equal(reviewed.review.verdict, 'fail');
  assert.equal(reviewed.review.completed, false);
  assert.equal(reviewed.review.independentVerification.status, 'incomplete');
});
