import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { writeAsk } from '../src/lib/ask.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { loadAvailableMetrics } from '../src/lib/metrics.mjs';
import { applyFeedback } from '../src/planner/feedback.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { readTaskMetadata } from '../src/runtime/estimate.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { runPlanner } from '../src/seats/planner.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const example = readFileSync(join(root, 'roster.config.example.yml'), 'utf8');
const config = parseConfig(example);
const record = (session, values = {}) => ({
  session, model: 'observed-model', task_class: 'fix', effort: 'h',
  verdict: 'accept', difficulty: 4, again: true, minutes: 10, comment: '', ...values,
});

function fixture(t) {
  const base = mkdtempSync(join(tmpdir(), 'roster-feedback-'));
  t.after(() => rmSync(base, { recursive: true, force: true }));
  const repoRoot = join(base, 'roster');
  const learningRoot = join(base, 'project');
  mkdirSync(repoRoot);
  mkdirSync(learningRoot);
  cpSync(join(root, 'skills'), join(repoRoot, 'skills'), { recursive: true });
  cpSync(join(root, 'principals'), join(repoRoot, 'principals'), { recursive: true });
  writeFileSync(join(repoRoot, 'roster.config.example.yml'), example);
  const evalsFile = join(learningRoot, '.roster', 'evals.jsonl');
  return {
    repoRoot, learningRoot, evalsFile,
    history(records) {
      mkdirSync(join(learningRoot, '.roster'), { recursive: true });
      writeFileSync(evalsFile, records.map((value) => JSON.stringify(value)).join('\n') + '\n');
    },
    worktree(number) {
      const worktree = join(learningRoot, `work-${number}`);
      mkdirSync(worktree);
      writeFileSync(join(worktree, 'AGENTS.md'), '# Instructions\nStay within task scope.\n');
      writeFileSync(join(worktree, 'README.md'), '# Example\n');
      return worktree;
    },
  };
}

async function plan(options, number, activeConfig = config) {
  const worktree = options.worktree(number);
  const planner = await runPlanner({
    worktree, repoRoot: options.repoRoot, learningRoot: options.learningRoot,
    config: activeConfig, env: {},
    issue: { number, title: 'fix: README typo', body: 'Fix README.md.' },
    fetchImpl: () => assert.fail('Stub planner must not request a model'),
  });
  return { worktree, planner };
}

test('the first stub task is a baseline and one human acceptance seeds the next TASK model and context', async (t) => {
  const options = fixture(t);
  const first = await plan(options, 1);
  assert.equal(first.planner.metadata.model, '');
  assert.equal(first.planner.metadata.estimate_min, 15);
  assert.equal(first.planner.feedback, null);
  options.history([record('first-delivery')]);
  const before = readFileSync(options.evalsFile);
  const next = await plan(options, 2);
  assert.match(readFileSync(next.planner.taskPath, 'utf8'), /model: observed-model\n/);
  assert.equal(next.planner.feedback.source, 'prior-accept');
  assert.equal(next.planner.feedback.recommendation, null);
  assert.equal(next.planner.metadata.estimate_min, 15);
  const result = await runCoder({
    worktree: next.worktree, repoRoot: options.repoRoot, config, env: {},
    task: 'issue-2', session: 'roster-2-coder', priorFeedback: next.planner.feedback.context,
    fetchImpl: () => assert.fail('Stub coder must not request a model'),
    runTestCommand: () => assert.fail('Stub coder must not run tests'),
  });
  assert.equal(result.mode, 'stub');
  const context = readFileSync(join(next.worktree, 'CONTEXT.md'), 'utf8');
  assert.match(context, /## Prior feedback\n\n/);
  assert.match(context, /Last human verdict: accept/);
  assert.match(context, /insufficient data for a capacity recommendation/);
  assert.deepEqual(readFileSync(options.evalsFile), before);
});

test('three samples choose model, effort and the recommended accepted-time estimate, not another effort history', async (t) => {
  const options = fixture(t);
  options.history([
    ...[10, 20, 30].map((minutes, index) => record(`high-${index}`, { minutes })),
    ...[1000, 2000, 3000].map((minutes, index) => record(`low-${index}`, {
      minutes, effort: 'l', verdict: index === 2 ? 'rework' : 'accept',
    })),
  ]);
  const selected = await plan(options, 3, { ...config, llm: { ...config.llm, model: 'config-default' } });
  assert.equal(selected.planner.metadata.model, 'observed-model');
  assert.equal(selected.planner.metadata.estimate_min, 20);
  assert.equal(selected.planner.metadata.source, 'recommendation');
  assert.equal(selected.planner.feedback.effort, 'h');
  assert.equal(selected.planner.feedback.recommendation.n, 3);
  assert.match(selected.planner.estimate, /Recommendation samples: 3/);
  assert.match(selected.planner.task, /^skills: \[implement-task, run-tests, read-before-write, small-diff, result-report\]$/m);
});

test('explicit task models and configured baselines are preserved below the capacity threshold', async (t) => {
  const options = fixture(t);
  options.history([record('one')]);
  const configured = await plan(options, 4, { ...config, llm: { ...config.llm, model: 'configured-model' } });
  assert.equal(configured.planner.metadata.model, 'configured-model');
  options.history([record('one'), record('two'), record('three')]);
  const task = planStub('Fix README.md.', {
    title: 'fix: typo', metadata: { model: 'pinned-model', estimate_min: 42 },
  }).task;
  const pinned = applyFeedback(task, { learningRoot: options.learningRoot, config, env: {} });
  assert.equal(readTaskMetadata(pinned.task).model, 'pinned-model');
  assert.equal(readTaskMetadata(pinned.task).estimate_min, 42);
  assert.equal(pinned.feedback.recommendation, null);
  assert.equal(pinned.feedback.effort, null);
});

test('only the last relevant reject/rework comment is copied, with credentials removed and multiline text quoted', (t) => {
  const options = fixture(t);
  const token = `ghp_${'A'.repeat(30)}`;
  const apiToken = `sk-${'B'.repeat(30)}`;
  const comment = `Handle null input.\nCUSTOM_KEY=env-private-value\npassword="quoted value"\n` +
    `token='two words'\nBearer bearer-value\nhttps://name:private-pass@example.invalid/docs\n${token}\n${apiToken}\n` +
    '-----BEGIN PRIVATE KEY-----\nprivate-body\n-----END PRIVATE KEY-----';
  options.history([
    record('old', { verdict: 'reject', comment: 'Old feedback' }),
    record('latest', { verdict: 'rework', comment }),
    record('accepted'),
    record('unrelated', { task_class: 'docs', verdict: 'reject', comment: 'Unrelated feedback' }),
  ]);
  const before = readFileSync(options.evalsFile);
  const result = applyFeedback(planStub('Fix README.md.', { title: 'fix: typo' }).task, {
    learningRoot: options.learningRoot, config: { ...config, llm: { ...config.llm, api_key_env: 'CUSTOM_KEY' } },
    env: { CUSTOM_KEY: 'env-private-value' },
  });
  assert.match(result.feedback.context, /> Handle null input\./);
  for (const secret of ['env-private-value', 'quoted value', 'two words', 'bearer-value',
    'private-pass', token, apiToken, 'private-body', 'Unrelated feedback', 'Old feedback']) {
    assert.equal(result.feedback.context.includes(secret), false, secret);
  }
  assert.match(result.feedback.context, /redacted/);
  assert.deepEqual(readFileSync(options.evalsFile), before);
});

test('unrelated classes and failed deliveries do not seed a baseline; unsafe model metadata fails explicitly', (t) => {
  const options = fixture(t);
  const task = planStub('Fix README.md.', { title: 'fix: typo' }).task;
  options.history([record('docs-only', { task_class: 'docs' })]);
  assert.equal(applyFeedback(task, { learningRoot: options.learningRoot, config, env: {} }).feedback, null);
  options.history([record('failed', { excellence: 'fail' })]);
  assert.equal(readTaskMetadata(applyFeedback(task, { learningRoot: options.learningRoot, config, env: {} }).task).model, '');
  options.history([record('corrected'), record('corrected', { verdict: 'reject', comment: 'Wrong breakdown.' })]);
  assert.equal(readTaskMetadata(applyFeedback(task, { learningRoot: options.learningRoot, config, env: {} }).task).model, '');
  options.history([record('unsafe', { model: `ghp_${'C'.repeat(30)}` })]);
  assert.throws(() => applyFeedback(task, { learningRoot: options.learningRoot, config, env: {} }),
    /secret-like content/);
});

test('offline draft asks also load prior models and malformed history is not ignored', async (t) => {
  const options = fixture(t);
  mkdirSync(join(options.repoRoot, '.roster'), { recursive: true });
  writeFileSync(join(options.repoRoot, '.roster', 'evals.jsonl'), JSON.stringify(record('draft-prior')) + '\n');
  const draft = await writeAsk('fix: Update README.md.', {
    repoRoot: options.repoRoot, config, env: {}, id: 'feedback-draft',
  });
  assert.match(readFileSync(draft.taskPath, 'utf8'), /model: observed-model\n/);
  options.history([record('valid')]);
  writeFileSync(options.evalsFile, '{\n');
  await assert.rejects(plan(options, 5), /evals\.jsonl:1: invalid JSON/);
});

test('available history distinguishes an unborn repository from an exporter failure', (t) => {
  const options = fixture(t);
  options.history([record('local-only')]);
  mkdirSync(join(options.learningRoot, '.git'));
  assert.equal(loadAvailableMetrics({
    cwd: options.learningRoot, run: () => { throw Object.assign(new Error('No HEAD'), { status: 1 }); },
  }).length, 1);
  assert.throws(() => loadAvailableMetrics({
    cwd: options.learningRoot,
    run(program) {
      if (program === 'git') return 'a'.repeat(40);
      throw Object.assign(new Error('Broken export'), { status: 1 });
    },
  }), /Metrics exporter failed.*Broken export/);
});
