import { median } from './learn.mjs';

export const DELIVERY_GATES = ['red-green', 'shadow', 'self-review', 'verifying-reviewer'];
const unknown = 'unknown';
const label = (value) => typeof value === 'string' && value && value !== '-' ? value : unknown;

export function validateDelivery(value, source = 'Delivery evidence') {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`${source}: delivery must be an object`);
  }
  const fields = new Set(['id', 'attempt', 'hardware', 'duration_ms', 'estimate_min',
    'ask_created_at', 'pr_merged_at', 'review_verdict', 'review_repairs', 'gates']);
  for (const key of Object.keys(value)) {
    if (!fields.has(key)) throw new TypeError(`${source}: unsupported delivery field ${key}`);
  }
  if (typeof value.id !== 'string' || !/^[A-Za-z0-9._-]{1,64}$/.test(value.id)) {
    throw new TypeError(`${source}: delivery id must be an opaque identifier`);
  }
  if (!Number.isSafeInteger(value.attempt) || value.attempt < 1) {
    throw new TypeError(`${source}: delivery attempt must be a positive integer`);
  }
  if (value.hardware != null && (typeof value.hardware !== 'string' || !value.hardware.trim() ||
      value.hardware.length > 120 || /[\x00-\x1f\x7f]/.test(value.hardware))) {
    throw new TypeError(`${source}: delivery hardware must be bounded plain text`);
  }
  for (const field of ['duration_ms', 'estimate_min', 'review_repairs']) {
    if (value[field] != null && (!Number.isFinite(value[field]) || value[field] < 0 ||
        field !== 'duration_ms' && !Number.isSafeInteger(value[field]))) {
      throw new TypeError(`${source}: delivery ${field} must be nonnegative`);
    }
  }
  for (const field of ['ask_created_at', 'pr_merged_at']) {
    if (value[field] != null && (typeof value[field] !== 'string' ||
        !Number.isFinite(Date.parse(value[field])) || new Date(value[field]).toISOString() !== value[field])) {
      throw new TypeError(`${source}: delivery ${field} must be an ISO timestamp`);
    }
  }
  if (value.ask_created_at && value.pr_merged_at &&
      Date.parse(value.pr_merged_at) < Date.parse(value.ask_created_at)) {
    throw new TypeError(`${source}: PR merge precedes ask creation`);
  }
  if (value.review_verdict != null && !['pass', 'fail', 'escalate'].includes(value.review_verdict)) {
    throw new TypeError(`${source}: delivery review_verdict must be pass, fail, or escalate`);
  }
  if (value.gates != null) {
    if (typeof value.gates !== 'object' || Array.isArray(value.gates)) {
      throw new TypeError(`${source}: delivery gates must be an object`);
    }
    for (const [gate, counts] of Object.entries(value.gates)) {
      if (!DELIVERY_GATES.includes(gate) || !counts || Object.keys(counts).some((key) =>
        !['checks', 'failures'].includes(key)) || !Number.isSafeInteger(counts.checks) ||
        counts.checks < 1 || !Number.isSafeInteger(counts.failures) || counts.failures < 0 ||
        counts.failures > counts.checks) {
        throw new TypeError(`${source}: invalid delivery gate counts`);
      }
    }
  }
}

export function deliveryMetrics(records) {
  if (!Array.isArray(records)) throw new TypeError('Expected an array of delivery records');
  const groups = new Map();
  for (const record of records) {
    if (!record || typeof record !== 'object' || Array.isArray(record)) {
      throw new TypeError('Expected delivery record objects');
    }
    if (record.delivery != null) validateDelivery(record.delivery);
    const dimensions = [label(record.model), label(record.delivery?.hardware), label(record.seat)];
    const key = JSON.stringify(dimensions);
    if (!groups.has(key)) groups.set(key, { dimensions, records: new Map() });
    const identity = record.delivery
      ? JSON.stringify([record.delivery.id, record.delivery.attempt])
      : record.sha ? `sha:${record.sha.toLowerCase()}` : `row:${groups.get(key).records.size}`;
    // A later journal snapshot enriches an attempt rather than counting it again.
    groups.get(key).records.set(identity, record);
  }
  return [...groups.entries()].sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0)
    .map(([, group]) => {
      const [model, hardware, seat] = group.dimensions;
      const lead = new Map();
      const cycles = new Map();
      const reviews = new Map();
      const evaluations = new Map();
      const estimates = new Map();
      let repairs = 0;
      let repairSamples = 0;
      const gateChecks = Object.fromEntries(DELIVERY_GATES.map((gate) => [gate, 0]));
      const gateFailures = Object.fromEntries(DELIVERY_GATES.map((gate) => [gate, 0]));
      for (const record of group.records.values()) {
        const evidence = record.delivery;
        if (evidence?.ask_created_at && evidence.pr_merged_at) {
          lead.set(evidence.id, (Date.parse(evidence.pr_merged_at) - Date.parse(evidence.ask_created_at)) / 60000);
        }
        if (seat === 'coder' && evidence?.duration_ms != null) {
          cycles.set(evidence.id, (cycles.get(evidence.id) ?? 0) + evidence.duration_ms / 60000);
        }
        if (evidence?.review_verdict) {
          const previous = reviews.get(evidence.id);
          if (!previous || evidence.attempt < previous.attempt) reviews.set(evidence.id, evidence);
        }
        if (evidence?.review_repairs != null) {
          repairs += evidence.review_repairs;
          repairSamples += 1;
        }
        if (record.evaluation) {
          const evaluation = record.evaluation;
          const key = evaluation.sha?.toLowerCase() ?? evaluation.session;
          if (key) evaluations.set(key, evaluation);
          if (evidence?.estimate_min != null && Number.isFinite(evaluation.minutes) && evaluation.minutes >= 0) {
            if (key && !estimates.has(key)) estimates.set(key, evaluation.minutes - evidence.estimate_min);
          }
        }
        for (const [gate, counts] of Object.entries(evidence?.gates ?? {})) {
          gateChecks[gate] += counts.checks;
          gateFailures[gate] += counts.failures;
        }
      }
      const reworkEvals = [...evaluations.values()].filter(({ verdict }) => verdict === 'rework').length;
      return {
        model, hardware, seat, runs: group.records.size,
        leadMinutes: median([...lead.values()]) ?? unknown,
        coderMinutes: median([...cycles.values()]) ?? unknown,
        firstPassYield: reviews.size ? [...reviews.values()].filter((review) =>
          review.attempt === 1 && review.review_verdict === 'pass').length / reviews.size : unknown,
        rework: repairSamples || evaluations.size ? repairs + reworkEvals : unknown,
        gateFailures: Object.fromEntries(DELIVERY_GATES.map((gate) =>
          [gate, gateChecks[gate] ? gateFailures[gate] : unknown])),
        estimateErrorMinutes: median([...estimates.values()]) ?? unknown,
        samples: { lead: lead.size, cycle: cycles.size, review: reviews.size,
          repairs: repairSamples, evaluations: evaluations.size, estimate: estimates.size },
        gateChecks,
      };
    });
}

export function formatDeliveryMetrics(groups) {
  if (!Array.isArray(groups)) throw new TypeError('Expected delivery metrics groups');
  if (!groups.length) return 'No delivery records found.\n';
  const number = (value) => typeof value === 'number' ? String(Number(value.toFixed(3))) : value;
  const rows = [
    ['MODEL', 'HARDWARE', 'SEAT', 'RUNS', 'LEAD_MIN', 'CODER_MIN', 'FIRST_PASS', 'REWORK', 'ESTIMATE_ERROR_MIN',
      ...DELIVERY_GATES],
    ...groups.map((group) => [group.model, group.hardware, group.seat, String(group.runs),
      number(group.leadMinutes), number(group.coderMinutes),
      typeof group.firstPassYield === 'number' ? `${(group.firstPassYield * 100).toFixed(1)}%` : unknown,
      String(group.rework), number(group.estimateErrorMinutes),
      ...DELIVERY_GATES.map((gate) => String(group.gateFailures[gate]))]),
  ];
  const widths = rows[0].map((_, index) => Math.max(...rows.map((row) => row[index].length)));
  return `${rows.map((row) => row.map((cell, index) =>
    index === row.length - 1 ? cell : cell.padEnd(widths[index])).join('  ')).join('\n')}\n`;
}

export function parseStatsOptions(args) {
  const options = {};
  const seen = new Set();
  const usage = 'Use roster stats [--delivery [--json]] [--ref REVISION_OR_RANGE] [--evals PATH].';
  for (let index = 0; index < args.length; index += 1) {
    const flag = args[index];
    if (seen.has(flag)) throw new TypeError(usage);
    seen.add(flag);
    if (flag === '--delivery' || flag === '--json') options[flag.slice(2)] = true;
    else if (flag === '--ref' || flag === '--evals') {
      const value = args[++index];
      if (!value || value.startsWith('-')) throw new TypeError(usage);
      options[flag === '--ref' ? 'ref' : 'evalsPath'] = value;
    } else throw new TypeError(usage);
  }
  if (options.json && !options.delivery) throw new TypeError(usage);
  return options;
}
