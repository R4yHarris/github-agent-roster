import { TASK_CLASSES, SHA } from './learn.mjs';

export const PILOT_LIMITS = Object.freeze({
  bytes: 2 * 1024 * 1024, tasks: 1000, records: 20000, trials: 100,
  depth: 12, nodes: 200000, string: 4096,
});
const METRICS = ['prompt_tokens', 'completion_tokens', 'latency_ms', 'energy_joules', 'cost_cents'];
const compare = (a, b) => a < b ? -1 : a > b ? 1 : 0;
const object = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

export class PilotsError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'PilotsError';
    this.code = code;
  }
}

function requireValue(ok, path, message, code = 'invalid-input') {
  if (!ok) throw new PilotsError(code, `${path}: ${message}`);
}

function boundedJson(value) {
  let nodes = 0;
  let bytes = 0;
  const ancestors = new Set();
  function visit(item, depth) {
    requireValue(++nodes <= PILOT_LIMITS.nodes && depth <= PILOT_LIMITS.depth,
      'input', 'JSON exceeds node/depth bounds');
    if (typeof item === 'string') {
      requireValue(item.length <= PILOT_LIMITS.string, 'input', 'string exceeds length bound');
      bytes += Buffer.byteLength(item, 'utf8') + 2;
    } else if (item !== null && typeof item === 'object') {
      requireValue(!ancestors.has(item), 'input', 'cyclic JSON');
      requireValue(Array.isArray(item) || Object.getPrototypeOf(item) === Object.prototype ||
        Object.getPrototypeOf(item) === null, 'input', 'expected plain JSON objects');
      ancestors.add(item);
      bytes += 2;
      for (const [key, child] of Object.entries(item)) {
        bytes += Buffer.byteLength(key, 'utf8') + 4;
        visit(child, depth + 1);
      }
      ancestors.delete(item);
    } else {
      requireValue(item === null || typeof item === 'boolean' ||
        (typeof item === 'number' && Number.isFinite(item)), 'input', 'expected finite JSON values');
      bytes += 24;
    }
    requireValue(bytes <= PILOT_LIMITS.bytes, 'input', 'JSON exceeds byte bound');
  }
  visit(value, 0);
  requireValue(Buffer.byteLength(JSON.stringify(value), 'utf8') <= PILOT_LIMITS.bytes,
    'input', 'JSON exceeds byte bound');
}

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (!object(value)) return value;
  return Object.fromEntries(Object.keys(value).sort(compare).map((key) => [key, canonical(value[key])]));
}

const equal = (a, b) => JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
function text(value, path) {
  requireValue(typeof value === 'string' && value.trim() === value && value.length > 0 &&
    value.length <= 256 && !/[\x00-\x1f\x7f]/.test(value), path, 'expected nonempty bounded text');
}
function identifier(value, path) {
  requireValue(typeof value === 'string' && /^(?!-$)[A-Za-z0-9._-]{1,64}$/.test(value),
    path, 'expected opaque 1-64 character identifier');
}
function boolean(value, path) {
  requireValue(typeof value === 'boolean', path, 'expected boolean');
}
function array(value, path, max) {
  requireValue(Array.isArray(value) && value.length <= max, path, `expected array of at most ${max} entries`);
}
function number(value, path, integer = false) {
  requireValue(Number.isFinite(value) && value >= 0 && value <= Number.MAX_SAFE_INTEGER &&
    (!integer || Number.isSafeInteger(value)),
    path, `expected nonnegative ${integer ? 'safe integer' : 'finite number'}`);
}
function controls(value, path) {
  requireValue(object(value), path, 'expected shared controls object');
  requireValue(object(value.sampler) && Object.keys(value.sampler).length > 0,
    `${path}.sampler`, 'expected nonempty frozen sampler object');
  for (const [key, setting] of Object.entries(value.sampler)) {
    requireValue(typeof setting === 'boolean' || typeof setting === 'string' ||
      (typeof setting === 'number' && Number.isFinite(setting)), `${path}.sampler.${key}`,
    'expected explicit scalar sampler setting, not unknown/nested data');
    if (['temperature', 'top_p', 'top_k', 'seed'].includes(key)) {
      number(setting, `${path}.sampler.${key}`, ['seed', 'top_k'].includes(key));
      if (key === 'top_p') requireValue(setting > 0 && setting <= 1, `${path}.sampler.top_p`, 'expected (0,1]');
    }
  }
  requireValue(object(value.test_budget), `${path}.test_budget`, 'expected test budget object');
  number(value.test_budget.calls, `${path}.test_budget.calls`, true);
  number(value.test_budget.timeout_ms, `${path}.test_budget.timeout_ms`, true);
  array(value.tools, `${path}.tools`, 100);
  value.tools.forEach((tool, index) => text(tool, `${path}.tools[${index}]`));
  requireValue(new Set(value.tools).size === value.tools.length, `${path}.tools`, 'duplicate tool identity');
  text(value.source_evidence, `${path}.source_evidence`);
}

export function validatePilotManifest(manifest) {
  boundedJson(manifest);
  requireValue(object(manifest), 'manifest', 'expected object');
  requireValue(manifest.schema === 1, 'manifest.schema', 'expected schema 1');
  boolean(manifest.synthetic, 'manifest.synthetic');
  array(manifest.conditions, 'manifest.conditions', 2);
  requireValue(manifest.conditions.length === 2, 'manifest.conditions',
    'exactly one baseline and one eve required', 'invalid-condition');
  const kinds = new Set();
  const ids = new Set();
  for (const [index, condition] of manifest.conditions.entries()) {
    const path = `manifest.conditions[${index}]`;
    requireValue(object(condition), path, 'expected condition object', 'invalid-condition');
    identifier(condition.id, `${path}.id`);
    requireValue(['baseline', 'eve'].includes(condition.kind), `${path}.kind`,
      'expected baseline or eve', 'invalid-condition');
    requireValue(!ids.has(condition.id) && !kinds.has(condition.kind), path,
      'duplicate condition identity or kind', 'duplicate-identity');
    ids.add(condition.id);
    kinds.add(condition.kind);
    text(condition.model, `${path}.model`);
    text(condition.profile, `${path}.profile`);
    requireValue(object(condition.behavior_bundle), `${path}.behavior_bundle`, 'expected frozen bundle object');
    identifier(condition.behavior_bundle.id, `${path}.behavior_bundle.id`);
    controls(condition.shared_controls, `${path}.shared_controls`);
  }
  const [baseline, eve] = ['baseline', 'eve'].map((kind) =>
    manifest.conditions.find((condition) => condition.kind === kind));
  requireValue(baseline.behavior_bundle.id !== eve.behavior_bundle.id ||
    equal(baseline.behavior_bundle, eve.behavior_bundle), 'manifest.conditions.behavior_bundle',
  'one bundle identity cannot refer to different frozen bundles', 'duplicate-identity');
  for (const key of ['model', 'profile', 'shared_controls']) {
    requireValue(equal(baseline[key], eve[key]), `manifest.conditions.${key}`,
      'baseline/Eve shared conditions must match', 'mismatched-controls');
  }
  // The bundle is the deliberate experimental factor, not a shared control.
  array(manifest.tasks, 'manifest.tasks', PILOT_LIMITS.tasks);
  requireValue(manifest.tasks.length > 0, 'manifest.tasks', 'at least one task required');
  const tasks = new Set();
  for (const [index, task] of manifest.tasks.entries()) {
    const path = `manifest.tasks[${index}]`;
    requireValue(object(task), path, 'expected task object');
    identifier(task.id, `${path}.id`);
    requireValue(!tasks.has(task.id), `${path}.id`, 'duplicate task identity', 'duplicate-identity');
    tasks.add(task.id);
    requireValue(TASK_CLASSES.includes(task.class), `${path}.class`, 'expected docs, fix, feat or test');
    requireValue(Number.isInteger(task.difficulty) && task.difficulty >= 1 && task.difficulty <= 5,
      `${path}.difficulty`, 'expected integer 1-5');
    requireValue(typeof task.base_revision === 'string' && SHA.test(task.base_revision),
      `${path}.base_revision`, 'expected full 40/64-character hexadecimal SHA');
    for (const key of ['model', 'profile']) {
      requireValue(task[key] === baseline[key], `${path}.${key}`, 'must match condition', 'mismatched-controls');
    }
    requireValue(['task', 'fault'].includes(task.kind), `${path}.kind`, 'expected task or fault');
    boolean(task.held_out, `${path}.held_out`);
    boolean(task.synthetic, `${path}.synthetic`);
  }
  if (manifest.thresholds !== undefined) {
    array(manifest.thresholds, 'manifest.thresholds', METRICS.length);
    const seen = new Set();
    for (const [index, gate] of manifest.thresholds.entries()) {
      const path = `manifest.thresholds[${index}]`;
      requireValue(object(gate) && METRICS.includes(gate.metric), path, 'expected supported numeric metric');
      requireValue(!seen.has(gate.metric), path, 'duplicate threshold metric', 'duplicate-identity');
      seen.add(gate.metric);
      number(gate.max_eve_median, `${path}.max_eve_median`);
    }
  }
  return canonical(manifest);
}

function validateRecord(record, index, task, condition) {
  const path = `records[${index}]`;
  requireValue(object(record), path, 'expected outcome record');
  requireValue(Number.isSafeInteger(record.trial) && record.trial >= 1 && record.trial <= PILOT_LIMITS.trials,
    `${path}.trial`, `expected trial integer 1-${PILOT_LIMITS.trials}`);
  requireValue(object(record.context), `${path}.context`, 'expected frozen run context');
  const expected = {
    task_class: task.class, difficulty: task.difficulty, base_revision: task.base_revision.toLowerCase(),
    model: task.model, profile: task.profile, behavior_bundle: condition.behavior_bundle.id,
    shared_controls: condition.shared_controls,
  };
  for (const [key, value] of Object.entries(expected)) {
    const actual = key === 'base_revision' && typeof record.context[key] === 'string'
      ? record.context[key].toLowerCase() : record.context[key];
    requireValue(equal(actual, value), `${path}.context.${key}`,
      'does not match frozen task/condition', 'mismatched-controls');
  }
  if (record.synthetic !== undefined) boolean(record.synthetic, `${path}.synthetic`);
  requireValue(object(record.outcomes), `${path}.outcomes`, 'expected outcomes object');
  for (const [key, values] of [
    ['automated_tests', ['pass', 'fail']], ['review_verdict', ['pass', 'fail', 'escalate']],
  ]) {
    requireValue(record.outcomes[key] === undefined || record.outcomes[key] === null ||
      values.includes(record.outcomes[key]), `${path}.outcomes.${key}`, `expected ${values.join('/')} or null`);
  }
  const human = record.outcomes.human_acceptance;
  if (human != null) {
    requireValue(object(human) && human.source === 'human' &&
      ['accept', 'reject', 'rework'].includes(human.verdict), `${path}.outcomes.human_acceptance`,
    'expected explicit human source with accept/reject/rework verdict');
    text(human.evidence, `${path}.outcomes.human_acceptance.evidence`);
  }
  if (record.metrics != null) {
    requireValue(object(record.metrics), `${path}.metrics`, 'expected metrics object or null');
    for (const metric of METRICS) {
      if (record.metrics[metric] != null) number(record.metrics[metric],
        `${path}.metrics.${metric}`, metric.endsWith('_tokens'));
    }
    if (record.metrics.hardware != null) text(record.metrics.hardware, `${path}.metrics.hardware`);
  }
  for (const key of ['safety_violations', 'fault_violations']) {
    if (record[key] !== undefined) {
      array(record[key], `${path}.${key}`, 100);
      record[key].forEach((value, i) => text(value, `${path}.${key}[${i}]`));
    }
  }
}

export function evaluatePilotPair(input, records = input?.records) {
  const manifest = validatePilotManifest(input);
  array(records, 'records', PILOT_LIMITS.records);
  boundedJson(records);
  const tasks = new Map(manifest.tasks.map((task) => [task.id, task]));
  const conditions = new Map(manifest.conditions.map((condition) => [condition.id, condition]));
  const seen = new Set();
  const normalized = [];
  for (const [index, record] of records.entries()) {
    requireValue(object(record), `records[${index}]`, 'expected record object');
    const task = tasks.get(record.task_id);
    const condition = conditions.get(record.condition_id);
    requireValue(Boolean(task), `records[${index}].task_id`, 'unknown task identity', 'pairing-error');
    requireValue(Boolean(condition), `records[${index}].condition_id`, 'unknown condition identity', 'invalid-condition');
    validateRecord(record, index, task, condition);
    const identity = JSON.stringify([record.task_id, record.condition_id, record.trial]);
    requireValue(!seen.has(identity), `records[${index}]`, 'duplicate task/condition/trial identity', 'duplicate-identity');
    seen.add(identity);
    const synthetic = manifest.synthetic || task.synthetic || record.synthetic === true;
    normalized.push({
      task_id: task.id, condition_id: condition.id, condition: condition.kind, trial: record.trial,
      synthetic, automated_tests: record.outcomes.automated_tests ?? null,
      review_verdict: record.outcomes.review_verdict ?? null,
      human_acceptance: synthetic ? null : record.outcomes.human_acceptance?.verdict ?? null,
      human_evidence: synthetic ? null : record.outcomes.human_acceptance?.evidence ?? null,
      ignored_synthetic_human_acceptance: synthetic && record.outcomes.human_acceptance != null,
      metrics: Object.fromEntries([...METRICS, 'hardware'].map((metric) => [metric, record.metrics?.[metric] ?? null])),
      safety_violations: record.safety_violations ?? [], fault_violations: record.fault_violations ?? [],
    });
  }
  for (const task of manifest.tasks) {
    const trials = normalized.filter((record) => record.task_id === task.id);
    requireValue(trials.length > 0, `task ${task.id}`, 'missing baseline/Eve records', 'pairing-error');
    for (const trial of new Set(trials.map((record) => record.trial))) {
      requireValue(trials.filter((record) => record.trial === trial).length === 2, `task ${task.id} trial ${trial}`,
        'missing matching baseline/Eve trial', 'pairing-error');
    }
  }
  normalized.sort((a, b) => compare(a.task_id, b.task_id) || a.trial - b.trial || compare(a.condition, b.condition));
  return { manifest, records: normalized, source_records: canonical(records) };
}

const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length === 0 ? null : sorted.length % 2
    ? sorted[middle] : sorted[middle - 1] / 2 + sorted[middle] / 2;
};
function variability(values, total) {
  const known = values.filter((value) => value !== null);
  const mean = known.length ? known.reduce((sum, value) => sum + value / known.length, 0) : null;
  return {
    observed: known.length, unknown: total - known.length, mean,
    sample_standard_deviation: known.length < 2 ? null :
      Math.sqrt(known.reduce((sum, value) => sum + ((value - mean) ** 2) / (known.length - 1), 0)),
  };
}
function aggregate(records, field) {
  const values = records.map((record) => record[field]);
  if (field !== 'human_acceptance' && values.includes('fail')) return 'fail';
  return values.every((value) => value !== null && value === values[0]) ? values[0] : null;
}
function paired(tasks, field, positive) {
  const counts = {
    baseline_positive: 0, eve_positive: 0, comparable_tasks: 0, unknown_pairs: 0,
    both_positive: 0, neither_positive: 0, eve_only: 0, baseline_only: 0,
    difference_tasks: 0, difference_percentage_points: null, differences: [],
  };
  for (const task of tasks) {
    const baseline = task.baseline[field];
    const eve = task.eve[field];
    counts.baseline_positive += Number(baseline === positive);
    counts.eve_positive += Number(eve === positive);
    const difference = baseline === null || eve === null ? null :
      Number(eve === positive) - Number(baseline === positive);
    counts.differences.push({ task_id: task.task_id, baseline, eve, difference });
    if (difference === null) counts.unknown_pairs++;
    else {
      counts.comparable_tasks++;
      counts.difference_tasks += difference;
      counts[baseline === positive ? (eve === positive ? 'both_positive' : 'baseline_only')
        : eve === positive ? 'eve_only' : 'neither_positive']++;
    }
  }
  if (counts.comparable_tasks) counts.difference_percentage_points =
    100 * counts.difference_tasks / counts.comparable_tasks;
  return counts;
}

export function buildComparisonSummary(evaluation) {
  // Revalidate at the public boundary rather than trust a caller-built evaluation.
  requireValue(object(evaluation) && object(evaluation.manifest) && Array.isArray(evaluation.source_records),
    'evaluation', 'expected evaluatePilotPair result');
  const { manifest, records } = evaluatePilotPair(evaluation.manifest, evaluation.source_records);
  const summaries = [...manifest.tasks].sort((a, b) => compare(a.id, b.id)).map((task) => {
    const perTask = records.filter((record) => record.task_id === task.id);
    const result = { task_id: task.id, class: task.class, difficulty: task.difficulty,
      kind: task.kind, held_out: task.held_out, synthetic: perTask.some((record) => record.synthetic),
      trials_per_condition: perTask.length / 2 };
    for (const kind of ['baseline', 'eve']) {
      const trials = perTask.filter((record) => record.condition === kind);
      result[kind] = Object.fromEntries(['automated_tests', 'review_verdict', 'human_acceptance']
        .map((field) => [field, aggregate(trials, field)]));
      result[kind].metrics = Object.fromEntries(METRICS.map((metric) => {
        const values = trials.map((trial) => trial.metrics[metric]);
        return [metric, {
          median: values.every((value) => value !== null) ? median(values) : null,
          ...variability(values, trials.length),
        }];
      }));
    }
    return result;
  });
  const tasks = summaries.filter((task) => task.kind === 'task');
  const faults = summaries.filter((task) => task.kind === 'fault');
  const measured = (task) => task.held_out && !task.synthetic;
  const classes = Object.fromEntries(TASK_CLASSES.map((taskClass) => [taskClass, {
    observed: tasks.filter((task) => task.class === taskClass).length,
    measured_held_out: tasks.filter((task) => task.class === taskClass && measured(task)).length,
    target: 10,
  }]));
  const faultsMeasured = faults.filter(measured).length;
  const violations = (key) => records.flatMap((record) => record[key].map((violation) => ({
    task_id: record.task_id, condition_id: record.condition_id, trial: record.trial,
    synthetic: record.synthetic, violation,
  })));
  const safety = violations('safety_violations');
  const faultViolations = violations('fault_violations');
  const thresholds = (manifest.thresholds ?? []).map((gate) => {
    const values = tasks.map((task) => task.eve.metrics[gate.metric].median);
    const known = tasks.length > 0 && tasks.every(measured) && values.every((value) => value !== null);
    const observed = known ? median(values) : null;
    return { ...gate, observed_eve_median: observed,
      status: observed === null ? 'unknown' : observed <= gate.max_eve_median ? 'pass' : 'fail' };
  });
  return {
    schema: 1, interpretation: 'offline-comparison-only',
    source_conditions: ['baseline', 'eve'].map((kind) => {
      const condition = manifest.conditions.find((entry) => entry.kind === kind);
      return { condition: kind, id: condition.id, model: condition.model, profile: condition.profile,
        behavior_bundle: condition.behavior_bundle, shared_controls: condition.shared_controls };
    }),
    task_count: tasks.length,
    paired_counts: {
      automated_tests: paired(tasks, 'automated_tests', 'pass'),
      review_verdict: paired(tasks, 'review_verdict', 'pass'),
      human_acceptance: paired(tasks, 'human_acceptance', 'accept'),
    },
    coverage: {
      observed_tasks: tasks.length, measured_held_out_tasks: tasks.filter(measured).length,
      target_tasks: 40, classes, adversarial_observed: faults.length,
      adversarial_measured_held_out: faultsMeasured, adversarial_target: 20,
      status: Object.values(classes).every((value) => value.measured_held_out >= value.target) &&
        faultsMeasured >= 20 ? 'protocol-covered' : 'pilot-incomplete',
    },
    tasks, fault_cases: faults,
    trial_variability: records,
    unknown_metrics: records.flatMap((record) => [...METRICS, 'hardware'].filter((metric) =>
      record.metrics[metric] === null).map((metric) => ({
      task_id: record.task_id, condition_id: record.condition_id, trial: record.trial, metric,
    }))),
    safety_violations: safety, fault_violations: faultViolations,
    safety_gate: safety.length || faultViolations.length ? 'fail' : 'no-reported-violations',
    thresholds,
    uncertainty: 'Descriptive paired tasks and trial variability only; no independent-trial inference or superiority claim.',
  };
}
