import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { prepareBuiltinPublication } from '../src/lib/builtin.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { checkExcellence, snapshotWorktree, taskSkipsTests, writeResult } from '../src/runtime/excellence.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { withResearchSummary } from './helpers/research.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:3456/v1').replace('model: ""', 'model: local-model'));

async function fixture(context) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-excellence-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  const task = planStub('Update README.md.').task;
  writeFileSync(path.join(worktree, 'README.md'), '# Before\n');
  writeFileSync(path.join(worktree, 'TASK.md'), task);
  const baseline = await snapshotWorktree(worktree);
  return { repoRoot, worktree, task, baseline, env: {}, result: {
    mode: 'llm', model: 'local-model', turns: 2, tests: { exit_code: 0 },
    summary: 'README updated with test evidence.',
  } };
}

test('clean fixture passes with changed scope, executed tests, and recorded model/turns', async (context) => {
  const options = await fixture(context);
  writeFileSync(path.join(options.worktree, 'README.md'), '# After\n');
  const gate = await checkExcellence(options);
  const { snapshot, ...checks } = gate;
  assert.ok(snapshot instanceof Map);
  assert.deepEqual(checks, { pass: true, reasons: [], files: ['README.md'], model: 'local-model', turns: 2 });
  const file = await writeResult({ ...options, excellence: gate });
  assert.match(readFileSync(file, 'utf8'), /Checks: PASS/);
  assert.match(readFileSync(file, 'utf8'), /node --test exited 0/);
  assert.match(readFileSync(file, 'utf8'), /Model: local-model\nTool-loop turns: 2/);
});

test('a changed vendor submodule path fails the review even when tests pass', async (context) => {
  const options = await fixture(context);
  const git = (cwd, ...args) => execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.invalid',
    '-c', 'commit.gpgsign=false', ...args], { cwd, stdio: 'pipe' });
  const vendor = path.join(options.worktree, 'vendor', 'github-agent-contracts');
  mkdirSync(vendor, { recursive: true });
  git(vendor, 'init', '-q');
  writeFileSync(path.join(vendor, 'agent-pr.mjs'), 'first\n');
  git(vendor, 'add', '.');
  git(vendor, 'commit', '-q', '-m', 'first');
  git(options.worktree, 'init', '-q');
  git(options.worktree, 'add', 'README.md', 'TASK.md', 'vendor/github-agent-contracts');
  git(options.worktree, 'commit', '-q', '-m', 'base');
  options.baseline = await snapshotWorktree(options.worktree);
  writeFileSync(path.join(vendor, 'agent-pr.mjs'), 'second\n');
  git(vendor, 'commit', '-q', '-am', 'second');
  writeFileSync(path.join(options.worktree, 'README.md'), '# After\n');
  const gate = await checkExcellence(options);
  assert.equal(options.result.tests.exit_code, 0);
  assert.equal(gate.pass, false);
  assert.ok(gate.reasons.some((reason) => /vendor path.*vendor\/github-agent-contracts$/.test(reason)), gate.reasons.join('\n'));
});

test('secret paths and out-of-scope changes fail without reading or reporting secret values', async (context) => {
  const options = await fixture(context);
  writeFileSync(path.join(options.worktree, '.env'), 'PRIVATE=fixture-secret-value\n');
  writeFileSync(path.join(options.worktree, 'outside.txt'), 'unrelated');
  const gate = await checkExcellence(options);
  assert.equal(gate.pass, false);
  assert.ok(gate.reasons.some((reason) => reason.includes('.env')));
  assert.ok(gate.reasons.some((reason) => reason.includes('outside.txt')));
  const file = await writeResult({ ...options, excellence: gate });
  const report = readFileSync(file, 'utf8');
  assert.match(report, /Checks: FAIL[\s\S]*First failure:/);
  assert.doesNotMatch(report, /fixture-secret-value/);
});

test('known secret material in an allowed file fails without echoing the credential', async (context) => {
  const options = await fixture(context);
  options.env = { CUSTOM_KEY: 'fixture-only-private-value' };
  options.apiKeyEnv = 'CUSTOM_KEY';
  writeFileSync(path.join(options.worktree, 'README.md'), 'fixture-only-private-value');
  const gate = await checkExcellence(options);
  assert.equal(gate.pass, false);
  assert.match(gate.reasons.join('\n'), /Secret material.*README\.md/);
  assert.doesNotMatch(gate.reasons.join('\n'), /fixture-only-private-value/);
});

test('edits inside existing protected directories fail in non-Git fixtures', async (context) => {
  const options = await fixture(context);
  mkdirSync(path.join(options.worktree, '.github', 'workflows'), { recursive: true });
  const workflow = path.join(options.worktree, '.github', 'workflows', 'ci.yml');
  writeFileSync(workflow, 'original');
  options.baseline = await snapshotWorktree(options.worktree);
  writeFileSync(workflow, 'modified workflow');
  const gate = await checkExcellence(options);
  assert.equal(gate.pass, false);
  assert.ok(gate.reasons.some((reason) => reason.includes('.github/workflows/ci.yml')));
});

test('only initial task frontmatter can waive tests and metadata must still be recorded', async (context) => {
  const options = await fixture(context);
  options.result = { ...options.result, tests: undefined };
  assert.equal((await checkExcellence(options)).pass, false);
  const task = options.task.replace('---\n', '---\ntests: none\n');
  assert.equal(taskSkipsTests(task), true);
  assert.equal(taskSkipsTests(options.task + '\ntests: none\n'), false);
  assert.equal((await checkExcellence({ ...options, task })).pass, true);
  assert.equal((await checkExcellence({
    ...options, task, result: { ...options.result, model: '', turns: undefined },
  })).pass, false);
});

test('edits after final verification invalidate an earlier passing gate', async (context) => {
  const options = await fixture(context);
  const verified = await checkExcellence(options);
  assert.equal(verified.pass, true);
  writeFileSync(path.join(options.worktree, 'README.md'), '# Later change\n');
  const gate = await checkExcellence({ ...options, verifiedSnapshot: verified.snapshot });
  assert.equal(gate.pass, false);
  assert.match(gate.reasons.join('\n'), /changed after final verification/);
});

test('Git diff checks include out-of-scope changes that already existed before the loop', async (context) => {
  const options = await fixture(context);
  const git = (...args) => execFileSync('git', args, { cwd: options.worktree, encoding: 'utf8', stdio: 'pipe' });
  git('init', '--quiet');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '--quiet', '-m', 'Fixture');
  writeFileSync(path.join(options.worktree, 'outside.txt'), 'preexisting untracked change');
  options.baseline = await snapshotWorktree(options.worktree);
  const gate = await checkExcellence(options);
  assert.equal(gate.pass, false);
  assert.match(gate.reasons.join('\n'), /outside\.txt/);
});

test('an explicit no-tests task skips automatic tests but still writes a gated result', async (context) => {
  const options = await fixture(context);
  cpSync(new URL('../principals/', import.meta.url), path.join(options.repoRoot, 'principals'), { recursive: true });
  cpSync(new URL('../skills/', import.meta.url), path.join(options.repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(options.worktree, 'AGENTS.md'), '# Instructions\nStay scoped.\n');
  writeFileSync(path.join(options.worktree, 'TASK.md'), options.task.replace('---\n', '---\ntests: none\n'));
  const result = await runCoder({
    ...options, config, task: 'issue-4', session: 'coder-4', vault: { get: async () => undefined },
    fetchImpl: withResearchSummary(async () => ({ status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'No tests requested.' } }],
    }) })),
    runTestCommand: () => assert.fail('Explicit no-tests task must not automatically execute tests'),
  });
  assert.equal(result.excellence.pass, true);
  assert.equal(result.testsSkipped, true);
  assert.equal(result.tests, undefined);
  assert.match(readFileSync(result.resultPath, 'utf8'), /Tests explicitly waived/);
});

test('a test subprocess scope violation writes a failed result and cannot reach publication', async (context) => {
  const options = await fixture(context);
  cpSync(new URL('../principals/', import.meta.url), path.join(options.repoRoot, 'principals'), { recursive: true });
  cpSync(new URL('../skills/', import.meta.url), path.join(options.repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(options.worktree, 'AGENTS.md'), '# Instructions\nStay scoped.\n');
  let failure;
  await assert.rejects(runCoder({
    ...options, config, task: 'issue-4', session: 'coder-4', vault: { get: async () => undefined },
    fetchImpl: withResearchSummary(async () => ({ status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }],
    }) })),
    runTestCommand: async () => {
      writeFileSync(path.join(options.worktree, 'outside.txt'), 'test side effect');
      return { stdout: 'tests pass', stderr: '' };
    },
  }), (error) => { failure = error; return /outside TASK\.md/.test(error.message); });
  const report = readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8');
  assert.match(report, /Checks: FAIL[\s\S]*outside\.txt/);
  const memory = readFileSync(path.join(options.repoRoot, '.roster', 'memory', 'coder.jsonl'), 'utf8')
    .trimEnd().split('\n').map(JSON.parse);
  assert.equal(memory.at(-1).status, 'failed');
  await assert.rejects(prepareBuiltinPublication({
    worktreePath: options.worktree, planner: { task: options.task, recipe: 'recipe' },
    runs: { coder: { env: {} } }, result: failure.result,
  }, { config, env: {} }), /passing excellence gate/);
});
