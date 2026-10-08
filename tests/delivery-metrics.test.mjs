import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DELIVERY_GATES, deliveryMetrics, formatDeliveryMetrics, parseStatsOptions,
  validateDelivery } from '../src/lib/delivery-metrics.mjs';
import { loadLearning, recordRun, recordDeliveryPublication } from '../src/lib/learn.mjs';
import { loadMetrics } from '../src/lib/metrics.mjs';
import { materializeRun } from '../src/metrics/run.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';

const evidence = (id, fields = {}) => ({ id, attempt: 1, ...fields });
const row = (model, seat, delivery, evaluation) => ({ model, seat, delivery, evaluation });
const at = (minutes) => new Date(Date.UTC(2026, 0, 1, 0, minutes)).toISOString();

test('three-model ledger reports observed delivery timing, review yield, rework and gate counts', () => {
  const records = [
    row('alpha', 'coder', evidence('a', { hardware: 'local GPU', duration_ms: 120000,
      ask_created_at: at(0), pr_merged_at: at(30), estimate_min: 5, review_repairs: 0,
      gates: { shadow: { checks: 2, failures: 1 }, 'red-green': { checks: 1, failures: 0 } } }),
    { session: 'a', verdict: 'rework', minutes: 8 }),
    row('alpha', 'coder', evidence('a', { attempt: 2, hardware: 'local GPU', duration_ms: 60000,
      estimate_min: 5, review_repairs: 1 }), { session: 'a', verdict: 'rework', minutes: 8 }),
    row('alpha', 'reviewer', evidence('a', { hardware: 'local GPU', review_verdict: 'fail',
      gates: { 'verifying-reviewer': { checks: 1, failures: 1 } } })),
    row('alpha', 'reviewer', evidence('a', { attempt: 2, hardware: 'local GPU', review_verdict: 'pass' })),
    row('alpha', 'reviewer', evidence('b', { hardware: 'local GPU', review_verdict: 'pass',
      gates: { 'verifying-reviewer': { checks: 1, failures: 0 } } })),
    row('beta', 'coder', evidence('c', { hardware: 'cloud', duration_ms: 240000,
      ask_created_at: at(10), pr_merged_at: at(20), review_repairs: 0 })),
    row('gamma', 'reviewer', evidence('d', { review_verdict: 'escalate' })),
  ];
  const groups = deliveryMetrics(records);
  const alpha = groups.find(({ model, seat }) => model === 'alpha' && seat === 'coder');
  assert.equal(alpha.leadMinutes, 30);
  assert.equal(alpha.coderMinutes, 3);
  assert.equal(alpha.rework, 2);
  assert.equal(alpha.estimateErrorMinutes, 3);
  assert.equal(alpha.samples.estimate, 1);
  assert.equal(alpha.gateFailures.shadow, 1);
  assert.equal(alpha.gateFailures['red-green'], 0);
  assert.equal(alpha.gateFailures['self-review'], 'unknown');
  const review = groups.find(({ model, seat }) => model === 'alpha' && seat === 'reviewer');
  assert.equal(review.firstPassYield, 0.5);
  assert.equal(review.samples.review, 2);
  assert.equal(review.gateFailures['verifying-reviewer'], 1);
  assert.equal(groups.find(({ model }) => model === 'beta').leadMinutes, 10);
  assert.equal(groups.find(({ model }) => model === 'gamma').firstPassYield, 0);
  assert.match(formatDeliveryMetrics(groups), /FIRST_PASS/);
});

test('unknown evidence is distinct from measured zero and from skipped gates', () => {
  const [missing] = deliveryMetrics([{ model: 'model' }]);
  for (const field of ['leadMinutes', 'coderMinutes', 'firstPassYield', 'rework', 'estimateErrorMinutes']) {
    assert.equal(missing[field], 'unknown');
  }
  assert.equal(missing.hardware, 'unknown');
  assert.equal(missing.seat, 'unknown');
  assert.ok(DELIVERY_GATES.every((gate) => missing.gateFailures[gate] === 'unknown'));
  const [zero] = deliveryMetrics([row('model', 'coder', evidence('z', {
    duration_ms: 0, estimate_min: 0, review_repairs: 0, ask_created_at: at(0), pr_merged_at: at(0),
  }), { session: 'z', minutes: 0, verdict: 'accept' })]);
  assert.equal(zero.coderMinutes, 0);
  assert.equal(zero.leadMinutes, 0);
  assert.equal(zero.rework, 0);
  assert.equal(zero.estimateErrorMinutes, 0);
  assert.equal(formatDeliveryMetrics([]), 'No delivery records found.\n');
});

test('JSON is stable across independent row ordering and snapshots do not count attempts twice', () => {
  const rows = [row('zeta', 'coder', evidence('z', { duration_ms: 180000 })),
    row('alpha', 'reviewer', evidence('a', { review_verdict: 'pass' }))];
  assert.equal(JSON.stringify(deliveryMetrics(rows)), JSON.stringify(deliveryMetrics([...rows].reverse())));
  assert.equal(deliveryMetrics([rows[0], rows[0]])[0].runs, 1);
  assert.equal(deliveryMetrics([rows[0], rows[0]])[0].coderMinutes, 3);
  assert.equal(deliveryMetrics([row('alpha', 'reviewer',
    evidence('retry', { attempt: 2, review_verdict: 'pass' }))])[0].firstPassYield, 0);
});

test('stats flags preserve legacy selectors and reject ambiguous or unsupported input', () => {
  assert.deepEqual(parseStatsOptions(['--ref', 'HEAD', '--evals', 'evals.jsonl']), {
    ref: 'HEAD', evalsPath: 'evals.jsonl' });
  assert.deepEqual(parseStatsOptions(['--json', '--delivery']), { json: true, delivery: true });
  for (const args of [['--json'], ['--delivery', '--delivery'], ['--ref'], ['--evals', '--json'],
    ['--unknown'], ['--ref', '-bad']]) assert.throws(() => parseStatsOptions(args), /Use roster stats/);
});

test('malformed persisted delivery evidence fails explicitly', () => {
  for (const fields of [{ duration_ms: -1 }, { estimate_min: NaN }, { attempt: 0 },
    { hardware: 'private\ntext' }, { review_verdict: 'accept' }, { secret: 'not allowed' },
    { ask_created_at: at(2), pr_merged_at: at(1) }, { ask_created_at: 'yesterday' },
    { gates: { shadow: { checks: 1, failures: 2 } } },
    { gates: { shadow: { checks: 0, failures: 0 } } }]) {
    assert.throws(() => validateDelivery(evidence('bad', fields)), /delivery|merge/);
  }
  assert.throws(() => deliveryMetrics([null]), /record objects/);
});

test('publication caches verified timestamps and delivery loading preserves published coder attempts', async (t) => {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-delivery-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const measured = materializeRun({ provider: 'local', model: 'actual-model',
    session: 'coder', task: 'issue-42', effort: 'h' });
  const first = evidence('pipeline', { hardware: 'GPU', estimate_min: 5, duration_ms: 60000, review_repairs: 0 });
  const second = { ...first, attempt: 2, duration_ms: 120000, review_repairs: 1 };
  for (const delivery of [first, second]) {
    await recordRun({ session: 'coder', task: 'issue-42', seat: 'coder', delivery }, {
      cwd, run: measured, env: measured.env, createDirectory: true,
    });
  }
  const run = { repoRoot: cwd, task: 'issue-42', sessions: { coder: 'coder' },
    runs: { coder: measured }, delivery: { coder: second } };
  await recordDeliveryPublication(run, { deliveryTimestamps: {
    ask_created_at: at(0), pr_merged_at: at(30),
  } });
  assert.equal(loadLearning({ cwd }).runs.length, 3);
  const options = { cwd, contractsPath: resolveContractsPath(),
    run: () => `${JSON.stringify({ schema: 1, sha: 'a'.repeat(40), model: 'actual-model',
      session: 'coder', task: 'issue-42', effort: 'h' })}\n` };
  const [summary] = deliveryMetrics(loadMetrics({ ...options, delivery: true }));
  assert.equal(summary.runs, 2);
  assert.equal(summary.coderMinutes, 3);
  assert.equal(summary.rework, 1);
  assert.equal(summary.leadMinutes, 30);
  assert.equal(loadMetrics(options).length, 1, 'Legacy stats retains its commit-based return shape');
  assert.equal(deliveryMetrics(loadMetrics({ ...options, delivery: true, ref: 'HEAD' }))[0].coderMinutes, 3);
  const alternate = materializeRun({ provider: 'local', model: 'earlier-model',
    session: 'coder', task: 'issue-42', effort: 'h' });
  await recordRun({ session: 'coder', task: 'issue-42', seat: 'coder',
    delivery: evidence('earlier', { duration_ms: 60000 }) }, {
    cwd, run: alternate, env: alternate.env,
  });
  const earlier = loadMetrics({ ...options, delivery: true }).find(({ model }) => model === 'earlier-model');
  assert.ok(earlier, 'Final exported model must not replace earlier response-backed attribution');
  assert.equal(earlier.prompt_tokens, undefined);
  await assert.rejects(recordRun({ session: 'bad', delivery: evidence('bad', { duration_ms: -1 }) }, {
    cwd, env: {},
  }), /duration_ms/);
});
