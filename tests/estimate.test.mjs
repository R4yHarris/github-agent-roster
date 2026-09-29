import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { estimateTask, writeEstimate } from '../src/runtime/estimate.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('model: ""', 'model: served-model'));
const metadata = { task_class: 'fix', model: 'served-model' };
const evaluations = (minutes) => minutes.map((value, index) => ({
  session: `past-${index}`, model: 'served-model', task_class: 'fix',
  verdict: 'accept', difficulty: 3, again: true, minutes: value,
}));

test('first estimates use defaults and preserve explicit task estimates below three samples', () => {
  assert.deepEqual(estimateTask(), {
    difficulty: 2, estimate_min: 15, task_class: 'feat', model: '',
    source: 'task/default', n: 0, accepted: 0,
  });
  assert.equal(estimateTask(metadata, evaluations([10, 20])).estimate_min, 15);
  assert.equal(estimateTask({ ...metadata, estimate_min: 40 }, evaluations([10, 20])).estimate_min, 40);
  assert.match(planStub('Fix README.md.', { title: 'fix: typo' }).task,
    /difficulty: 2\nestimate_min: 15\ntask_class: fix\nmodel: \n/);
});

test('three matching evaluations use only accepted actuals and round medians to integer minutes', () => {
  assert.equal(estimateTask(metadata, evaluations([30, 10, 20])).estimate_min, 20);
  assert.equal(estimateTask(metadata, evaluations([1, 2, 3, 4])).estimate_min, 3);
  const mixed = evaluations([12, 999, 1000]);
  mixed[1].verdict = 'reject';
  mixed[2].verdict = 'rework';
  assert.equal(estimateTask(metadata, mixed).estimate_min, 12);
  const unsafe = evaluations([1, 10, 20]);
  unsafe[0].excellence = 'fail';
  assert.equal(estimateTask(metadata, unsafe).estimate_min, 15);
  assert.equal(estimateTask(metadata, mixed.map((record) => ({ ...record, verdict: 'reject' }))).estimate_min, 15);
});

test('estimation separates models/classes and counts corrected targets once', () => {
  const history = evaluations([10, 20, 90]);
  assert.equal(estimateTask({ ...metadata, model: 'other-model' }, history).n, 0);
  assert.equal(estimateTask({ ...metadata, task_class: 'docs' }, history).n, 0);
  const corrected = estimateTask(metadata, [...history, { ...history[2], verdict: 'rework' }]);
  assert.equal(corrected.n, 3);
  assert.equal(corrected.accepted, 2);
  assert.equal(corrected.estimate_min, 15);
  assert.equal(estimateTask(metadata, [history[0], history[0], history[0]]).n, 1);
  const linked = estimateTask(metadata, [...history,
    { ...history[2], sha: 'a'.repeat(40), verdict: 'rework' }]);
  assert.equal(linked.n, 3);
  assert.equal(linked.accepted, 2);
});

test('invalid metadata and timing fail explicitly instead of becoming defaults', () => {
  for (const invalid of [{ difficulty: 6 }, { difficulty: 0 }, { estimate_min: 1.5 },
    { estimate_min: -1 }, { task_class: 'chore' }, { model: 'unknown' }, { model: 'not a model' }]) {
    assert.throws(() => estimateTask(invalid), /Task/);
  }
  for (const minutes of [-1, 1.5, '10']) {
    assert.throws(() => estimateTask(metadata, [{ ...evaluations([1])[0], minutes }]), /minutes/);
  }
});

test('ESTIMATE.md and TASK fields use repository history, including legacy joined run metadata', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'roster-estimate-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const worktree = join(root, 'worktree');
  mkdirSync(worktree);
  mkdirSync(join(root, '.roster', 'runs'), { recursive: true });
  const history = evaluations([30, 10, 20]);
  const sha = 'a'.repeat(40);
  writeFileSync(join(root, '.roster', 'evals.jsonl'), history.map(({ model, task_class, ...evaluation }, index) =>
    JSON.stringify(index === 0 ? history[0] : evaluation)).join('\n') + '\n' +
    JSON.stringify({ ...history[0], session: undefined, sha, verdict: 'reject', minutes: 1000 }) + '\n');
  writeFileSync(join(root, '.roster', 'runs', 'runs.jsonl'), history.map(({ session, model, task_class }, index) =>
    JSON.stringify({ session, model, task_class, ...(index === 0 ? { sha } : {}) })).join('\n') + '\n');
  const source = planStub('Fix README.md.', { title: 'fix: typo' }).task;
  const result = await writeEstimate(source, { worktree, learningRoot: root, config, env: {} });
  assert.match(result.task, /^---\nskills: \[implement-task, run-tests, read-before-write, small-diff, result-report\]\n---\n# Task: fix: typo/);
  assert.match(result.task, /estimate_min: 15\ntask_class: fix\nmodel: served-model\n/);
  assert.equal(result.metadata.source, 'history');
  assert.equal(readFileSync(join(worktree, 'ESTIMATE.md'), 'utf8'), result.estimate);
  assert.match(result.estimate, /Matching timed evaluations: 3/);
  assert.match(result.estimate, /Accepted timed evaluations: 2/);
  await assert.rejects(writeEstimate(source, { worktree, learningRoot: root, config, env: {} }), /EEXIST/);
});

test('missing history is a baseline and malformed JSONL is not silently ignored', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'roster-estimate-baseline-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const source = '# Task: docs: guide\n\n## Ask\nmodel: malicious-model\n';
  const result = await writeEstimate(source, { worktree: root, learningRoot: root, config, env: {} });
  assert.match(result.task, /difficulty: 2\nestimate_min: 15\ntask_class: docs\nmodel: served-model\n/);
  assert.equal(result.metadata.n, 0);
  mkdirSync(join(root, '.roster'));
  writeFileSync(join(root, '.roster', 'evals.jsonl'), '{\n');
  await assert.rejects(writeEstimate(source, { worktree: root, learningRoot: root, config, env: {} }),
    /evals\.jsonl:1: invalid JSON/);
});
