import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import {
  PILOT_LIMITS, PilotsError, validatePilotManifest, evaluatePilotPair, buildComparisonSummary,
} from '../src/lib/eve-pilot.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const fixturePath = resolve(root, 'tests', 'fixtures', 'eve-pilot.json');
const fixture = () => JSON.parse(readFileSync(fixturePath, 'utf8'));
const summarize = (manifest) => buildComparisonSummary(evaluatePilotPair(manifest));
const cli = (...args) => spawnSync(process.execPath, [resolve(root, 'scripts', 'eve-pilot.mjs'), ...args],
  { cwd: root, encoding: 'utf8', timeout: 10000, maxBuffer: 4 * 1024 * 1024 });
const error = (fn, code, message) => assert.throws(fn, (value) =>
  value instanceof PilotsError && value.code === code && message.test(value.message));

function measuredFixture() {
  const manifest = fixture();
  manifest.synthetic = false;
  manifest.tasks.forEach((task) => { task.synthetic = false; });
  manifest.records.forEach((record) => {
    record.metrics = { prompt_tokens: 100, completion_tokens: 20, latency_ms: 800,
      energy_joules: 10, cost_cents: 0, hardware: 'observed-test-device' };
  });
  return manifest;
}

test('deterministic CLI JSON exactly matches the synthetic golden fixture', () => {
  const expected = JSON.parse(readFileSync(resolve(root, 'tests', 'fixtures', 'eve-pilot-summary.json'), 'utf8'));
  const first = cli('--manifest', fixturePath);
  const second = cli('--manifest', fixturePath);
  assert.equal(first.status, 0, first.stderr);
  assert.equal(first.stderr, '');
  assert.equal(first.stdout, second.stdout);
  assert.deepEqual(JSON.parse(first.stdout), expected);
  assert.deepEqual(summarize(fixture()), expected);
  assert.equal(expected.coverage.status, 'pilot-incomplete');
});

test('paired task counts do not count repeated trials or fault cases as tasks', () => {
  const result = summarize(fixture());
  assert.equal(result.task_count, 1);
  assert.equal(result.tasks[0].trials_per_condition, 2);
  assert.equal(result.trial_variability.length, 6);
  assert.equal(result.paired_counts.automated_tests.eve_positive, 1);
  assert.equal(result.paired_counts.automated_tests.eve_only, 1);
  assert.equal(result.paired_counts.automated_tests.difference_tasks, 1);
  assert.equal(result.paired_counts.automated_tests.difference_percentage_points, 100);
  assert.equal(result.fault_cases.length, 1);
  assert.equal(result.coverage.adversarial_observed, 1);
  assert.equal(result.coverage.classes.docs.observed, 1);
  assert.equal(result.coverage.measured_held_out_tasks, 0);
});

test('fixtures cannot supply human acceptance; tests, review and prose never infer it', () => {
  const manifest = fixture();
  manifest.records[0].outcomes.model_prose = 'The human accepted this task.';
  const result = summarize(manifest);
  assert.equal(result.paired_counts.human_acceptance.eve_positive, 0);
  assert.equal(result.paired_counts.human_acceptance.unknown_pairs, 1);
  assert.equal(result.tasks[0].eve.human_acceptance, null);
  assert.equal(result.trial_variability.find((record) =>
    record.ignored_synthetic_human_acceptance)?.human_acceptance, null);
  const real = measuredFixture();
  for (const record of real.records) delete record.outcomes.human_acceptance;
  assert.equal(summarize(real).paired_counts.human_acceptance.comparable_tasks, 0);
});

test('only explicit non-synthetic human evidence contributes; mixed trials stay unknown', () => {
  const manifest = measuredFixture();
  for (const record of manifest.records) {
    record.outcomes.human_acceptance = { source: 'human',
      verdict: record.condition_id === 'eve-v1' ? 'accept' : 'reject', evidence: 'operator-outcome-01' };
  }
  const accepted = summarize(manifest);
  assert.equal(accepted.paired_counts.human_acceptance.eve_positive, 1);
  assert.equal(accepted.paired_counts.human_acceptance.difference_tasks, 1);
  manifest.records[3].outcomes.human_acceptance.verdict = 'rework';
  assert.equal(summarize(manifest).tasks[0].eve.human_acceptance, null);
  manifest.records[3].outcomes.human_acceptance.verdict = 'accept';
  manifest.records[3].synthetic = true;
  assert.equal(summarize(manifest).paired_counts.human_acceptance.unknown_pairs, 1);
});

test('human markers require human source, compatible verdict and explicit evidence', () => {
  for (const human of ['accept', { source: 'model', verdict: 'accept', evidence: 'x' },
    { source: 'human', verdict: 'pass', evidence: 'x' }, { source: 'human', verdict: 'accept' }]) {
    const manifest = measuredFixture();
    manifest.records[0].outcomes.human_acceptance = human;
    error(() => summarize(manifest), 'invalid-input', /human_acceptance/);
  }
});

test('unknown metrics remain null with explicit unknown counts and cannot pass gates', () => {
  const result = summarize(fixture());
  assert.equal(result.thresholds[0].status, 'unknown');
  assert.equal(result.thresholds[0].observed_eve_median, null);
  const unknownTrial = result.trial_variability.find((record) => record.trial === 2);
  assert.deepEqual(unknownTrial.metrics, { prompt_tokens: null, completion_tokens: null,
    latency_ms: null, energy_joules: null, cost_cents: null, hardware: null });
  assert.equal(result.tasks[0].eve.metrics.latency_ms.median, null);
  assert.equal(result.tasks[0].eve.metrics.latency_ms.observed, 1);
  assert.equal(result.tasks[0].eve.metrics.latency_ms.unknown, 1);
  assert.equal(result.tasks[0].eve.metrics.latency_ms.sample_standard_deviation, null);
  assert.ok(result.unknown_metrics.some((value) => value.metric === 'hardware'));
  const measured = measuredFixture();
  delete measured.records[3].metrics.latency_ms;
  assert.equal(summarize(measured).thresholds[0].status, 'unknown');
});

test('measured zero is retained; fully measured gates pass/fail; synthetic numbers cannot pass', () => {
  const manifest = measuredFixture();
  assert.equal(summarize(manifest).thresholds[0].status, 'pass');
  manifest.thresholds[0].max_eve_median = 799;
  assert.equal(summarize(manifest).thresholds[0].status, 'fail');
  manifest.thresholds = [{ metric: 'cost_cents', max_eve_median: 0 }];
  assert.equal(summarize(manifest).thresholds[0].status, 'pass');
  assert.equal(summarize(manifest).thresholds[0].observed_eve_median, 0);
  manifest.synthetic = true;
  assert.equal(summarize(manifest).thresholds[0].status, 'unknown');
  manifest.synthetic = false;
  manifest.tasks[0].held_out = false;
  assert.equal(summarize(manifest).thresholds[0].status, 'unknown');
});

test('trial variability is descriptive sample deviation, not extra independent samples', () => {
  const manifest = measuredFixture();
  manifest.records[1].metrics.latency_ms = 600;
  manifest.records[3].metrics.latency_ms = 1000;
  const metric = summarize(manifest).tasks[0].eve.metrics.latency_ms;
  assert.equal(metric.observed, 2);
  assert.equal(metric.median, 800);
  assert.equal(metric.mean, 800);
  assert.equal(metric.sample_standard_deviation, Math.sqrt(80000));
});

test('safety and fault violations remain separately visible despite representative improvement', () => {
  const result = summarize(fixture());
  assert.equal(result.paired_counts.automated_tests.difference_tasks, 1);
  assert.equal(result.safety_gate, 'fail');
  assert.equal(result.safety_violations.length, 1);
  assert.equal(result.fault_violations.length, 1);
  assert.equal(result.safety_violations[0].task_id, 'fault-01');
  const manifest = fixture();
  delete manifest.records[5].safety_violations;
  delete manifest.records[5].fault_violations;
  assert.equal(summarize(manifest).safety_gate, 'no-reported-violations');
});

test('reject duplicate task, condition, bundle and trial identities explicitly', () => {
  const changes = [
    [(m) => m.tasks.push(structuredClone(m.tasks[0])), /duplicate task/],
    [(m) => { m.conditions[1].id = m.conditions[0].id; }, /duplicate condition/],
    [(m) => { m.conditions[1].kind = 'baseline'; }, /duplicate condition/],
    [(m) => { m.conditions[1].behavior_bundle.id = m.conditions[0].behavior_bundle.id; }, /bundle identity/],
    [(m) => m.records.push(structuredClone(m.records[0])), /duplicate task\/condition\/trial/],
  ];
  for (const [change, message] of changes) {
    const manifest = fixture();
    change(manifest);
    error(() => summarize(manifest), 'duplicate-identity', message);
  }
});

test('reject missing/unbalanced pairs, unknown tasks and unknown conditions', () => {
  const missing = fixture();
  missing.records.splice(1, 1);
  error(() => summarize(missing), 'pairing-error', /missing matching/);
  const noRecords = fixture();
  noRecords.records = noRecords.records.filter((record) => record.task_id !== 'docs-01');
  error(() => summarize(noRecords), 'pairing-error', /missing baseline/);
  const wrongTrial = fixture();
  wrongTrial.records[1].trial = 3;
  error(() => summarize(wrongTrial), 'pairing-error', /missing matching/);
  const unknownTask = fixture();
  unknownTask.records[0].task_id = 'not-a-task';
  error(() => summarize(unknownTask), 'pairing-error', /unknown task/);
  const unknownCondition = fixture();
  unknownCondition.records[0].condition_id = 'not-a-condition';
  error(() => summarize(unknownCondition), 'invalid-condition', /unknown condition/);
});

test('task, base, model, profile, bundle and every shared control must match snapshots', () => {
  for (const [key, value] of [
    ['task_class', 'fix'], ['difficulty', 2], ['base_revision', 'c'.repeat(40)],
    ['model', 'other-model'], ['profile', 'other-profile'], ['behavior_bundle', 'other-bundle'],
  ]) {
    const manifest = fixture();
    manifest.records[0].context[key] = value;
    error(() => summarize(manifest), 'mismatched-controls', new RegExp(key));
  }
  for (const [key, value] of [
    ['sampler', { temperature: 0.9 }], ['test_budget', { calls: 4, timeout_ms: 1000 }],
    ['tools', ['read']], ['source_evidence', 'other-sources'],
  ]) {
    for (const target of ['condition', 'record']) {
      const manifest = fixture();
      const controls = target === 'condition' ? manifest.conditions[1].shared_controls :
        manifest.records[0].context.shared_controls;
      controls[key] = value;
      error(() => summarize(manifest), 'mismatched-controls', /shared_controls/);
    }
  }
});

test('condition and manifest metadata cannot silently disagree', () => {
  for (const target of ['condition', 'task']) {
    for (const key of ['model', 'profile']) {
      const manifest = fixture();
      (target === 'condition' ? manifest.conditions[1] : manifest.tasks[0])[key] = 'different';
      error(() => summarize(manifest), 'mismatched-controls', new RegExp(key));
    }
  }
});

test('deliberate bundle differences and object-key order are valid; output order is stable', () => {
  const manifest = fixture();
  const expected = summarize(manifest);
  manifest.records.reverse();
  manifest.tasks.reverse();
  manifest.conditions.reverse();
  manifest.conditions[0].shared_controls.sampler = { seed: 7, temperature: 0.2 };
  assert.deepEqual(summarize(manifest), expected);
  assert.notEqual(expected.source_conditions[0].behavior_bundle.id, expected.source_conditions[1].behavior_bundle.id);
});

test('malformed manifests and bounded records fail clearly', () => {
  for (const change of [
    (m) => { m.schema = 2; }, (m) => { m.synthetic = 'true'; },
    (m) => { m.conditions[1].kind = 'unknown'; }, (m) => { m.conditions.pop(); },
    (m) => { delete m.conditions[0].behavior_bundle.id; },
    (m) => { m.tasks[0].class = 'research'; }, (m) => { m.tasks[0].difficulty = 0; },
    (m) => { m.tasks[0].base_revision = 'HEAD'; }, (m) => { m.tasks[0].held_out = 1; },
    (m) => { m.tasks[0].kind = 'unknown'; }, (m) => { m.records = null; },
    (m) => { m.records[0] = null; }, (m) => { m.records[0].trial = 0; },
    (m) => { m.records[0].trial = PILOT_LIMITS.trials + 1; },
    (m) => { m.records[0].outcomes = []; }, (m) => { m.records[0].outcomes.automated_tests = true; },
    (m) => { m.records[0].outcomes.review_verdict = 'accept'; },
    (m) => { m.records[0].metrics.latency_ms = -1; }, (m) => { m.records[0].metrics.energy_joules = 'unknown'; },
    (m) => { m.records[0].metrics.prompt_tokens = 0.5; },
    (m) => { m.records[0].metrics.latency_ms = Infinity; },
    (m) => { m.records[0].safety_violations = ['']; },
    (m) => { m.conditions[0].shared_controls.tools = ['read', 'read']; },
    (m) => { m.conditions[0].shared_controls.sampler.temperature = null; },
    (m) => { m.conditions[0].shared_controls.sampler.temperature = 'hot'; },
    (m) => { m.conditions[0].shared_controls.sampler.top_p = 1.1; },
    (m) => { m.conditions[0].shared_controls.sampler.seed = 0.5; },
    (m) => { m.conditions[0].shared_controls.test_budget.timeout_ms = -1; },
    (m) => { m.thresholds = [{ metric: 'intelligence', max_eve_median: 1 }]; },
    (m) => { m.thresholds[0].max_eve_median = -1; },
  ]) {
    const manifest = fixture();
    change(manifest);
    assert.throws(() => summarize(manifest), PilotsError);
  }
  const deep = fixture();
  let value = deep;
  for (let i = 0; i < 20; i++) value = value.extra = {};
  error(() => validatePilotManifest(deep), 'invalid-input', /bounds/);
  const cycle = fixture();
  cycle.extra = cycle;
  error(() => validatePilotManifest(cycle), 'invalid-input', /cyclic/);
  const large = fixture();
  large.extra = 'x'.repeat(PILOT_LIMITS.string + 1);
  error(() => validatePilotManifest(large), 'invalid-input', /length bound/);
  const many = fixture();
  many.records = Array(PILOT_LIMITS.records + 1).fill(null);
  assert.throws(() => summarize(many), PilotsError);
});

test('protocol coverage needs 10 measured held-out tasks per class and 20 separate faults', () => {
  const template = measuredFixture();
  const manifest = { ...template, tasks: [], records: [] };
  for (const taskClass of ['docs', 'fix', 'feat', 'test', 'fault']) {
    const count = taskClass === 'fault' ? 20 : 10;
    for (let i = 0; i < count; i++) {
      const task = { ...template.tasks[0], id: `${taskClass}-${i}`, class: taskClass === 'fault' ? 'test' : taskClass,
        kind: taskClass === 'fault' ? 'fault' : 'task' };
      manifest.tasks.push(task);
      for (const source of template.records.slice(0, 2)) {
        const record = structuredClone(source);
        record.task_id = task.id;
        record.context.task_class = task.class;
        manifest.records.push(record);
      }
    }
  }
  assert.equal(summarize(manifest).coverage.status, 'protocol-covered');
  assert.equal(summarize(manifest).task_count, 40);
  assert.equal(summarize(manifest).coverage.adversarial_measured_held_out, 20);
  manifest.tasks[0].held_out = false;
  assert.equal(summarize(manifest).coverage.status, 'pilot-incomplete');
  manifest.tasks[0].held_out = true;
  const fix = manifest.tasks.find((task) => task.class === 'fix');
  fix.class = 'docs';
  manifest.records.filter((record) => record.task_id === fix.id).forEach((record) => {
    record.context.task_class = 'docs';
  });
  assert.equal(summarize(manifest).coverage.status, 'pilot-incomplete');
  fix.class = 'fix';
  manifest.records.filter((record) => record.task_id === fix.id).forEach((record) => {
    record.context.task_class = 'fix';
  });
  manifest.tasks.at(-1).synthetic = true;
  assert.equal(summarize(manifest).coverage.status, 'pilot-incomplete');
  manifest.tasks.at(-1).synthetic = false;
  manifest.synthetic = true;
  assert.equal(summarize(manifest).coverage.status, 'pilot-incomplete');
});

test('public APIs are pure, do not mutate input, and accept separate bounded records', () => {
  const manifest = fixture();
  const before = structuredClone(manifest);
  const records = manifest.records;
  delete manifest.records;
  const expected = summarize(before);
  assert.deepEqual(buildComparisonSummary(evaluatePilotPair(manifest, records)), expected);
  assert.deepEqual(manifest, Object.fromEntries(Object.entries(before).filter(([key]) => key !== 'records')));
  assert.deepEqual(records, before.records);
  const evaluation = evaluatePilotPair(before);
  evaluation.records[0].human_acceptance = 'accept';
  evaluation.records[0].metrics.latency_ms = 0;
  assert.deepEqual(buildComparisonSummary(evaluation), expected);
  error(() => buildComparisonSummary({}), 'invalid-input', /evaluatePilotPair result/);
});

test('CLI help succeeds; arguments and I/O errors produce explicit stderr and empty stdout', () => {
  const help = cli('--help');
  assert.equal(help.status, 0);
  assert.equal(help.stderr, '');
  assert.match(help.stdout, /Usage:.*--manifest/);
  for (const args of [[], ['--bad'], ['--manifest'], ['--manifest', '--help'],
    ['--manifest', fixturePath, '--manifest', fixturePath], ['--help', '--records', fixturePath]]) {
    const result = cli(...args);
    assert.equal(result.status, 1);
    assert.equal(result.stdout, '');
    assert.match(result.stderr, /invalid-arguments/);
  }
  const missing = cli('--manifest', resolve(root, 'tests', 'fixtures', 'no-such-eve-pilot.json'));
  assert.equal(missing.status, 2);
  assert.match(missing.stderr, /io-error/);
  assert.equal(missing.stdout, '');
});

test('real CLI rejects invalid JSON, all pairing errors and supports records override without writes', () => {
  const scratch = resolve(root, 'tests', `.eve-pilot-test-${process.pid}`);
  mkdirSync(scratch);
  const manifestPath = resolve(scratch, 'manifest.json');
  const recordsPath = resolve(scratch, 'records.json');
  try {
    for (const change of [
      (m) => { m.records.pop(); },
      (m) => { m.records.push(structuredClone(m.records[0])); },
      (m) => { m.conditions[1].kind = 'invalid'; },
      (m) => { delete m.conditions[1].behavior_bundle; },
      (m) => { m.records[0].context.model = 'wrong'; },
      (m) => { m.records[0].outcomes.review_verdict = true; },
    ]) {
      const manifest = fixture();
      change(manifest);
      writeFileSync(manifestPath, JSON.stringify(manifest));
      const result = cli('--manifest', manifestPath);
      assert.equal(result.status, 1);
      assert.equal(result.stdout, '');
      assert.match(result.stderr, /eve-pilot: (pairing-error|duplicate-identity|invalid-condition|invalid-input|mismatched-controls):/);
    }
    writeFileSync(manifestPath, '{');
    assert.match(cli('--manifest', manifestPath).stderr, /invalid-json/);
    writeFileSync(manifestPath, ' '.repeat(PILOT_LIMITS.bytes + 1));
    assert.match(cli('--manifest', manifestPath).stderr, /byte bound/);
    const manifest = fixture();
    writeFileSync(recordsPath, JSON.stringify(manifest.records));
    delete manifest.records;
    const text = JSON.stringify(manifest);
    writeFileSync(manifestPath, text);
    const result = cli('--manifest', manifestPath, '--records', recordsPath);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(result.stdout), summarize(fixture()));
    assert.equal(readFileSync(manifestPath, 'utf8'), text);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
});
