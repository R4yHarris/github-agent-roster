// Provenance schema for machine-local durable history.
// Records describe runs, sessions, and tool evidence owned by the machine
// (outside the repository checkout). Records are append-safe and immutable:
// a record is frozen at creation, and history files are only ever appended.

export const SCHEMA_VERSION = '1.0.0';

/** Sentinel for metric fields whose value is not known. */
export const UNKNOWN_METRIC = 'unknown';

/**
 * Known metric fields. Known metrics carry a numeric value; anything else is
 * not a metric and must not be coerced to a number or silently dropped.
 */
export const KNOWN_METRICS = Object.freeze([
  'duration_ms',
  'cost_usd',
  'tokens_prompt',
  'tokens_completion',
  'tool_calls',
  'retry_count',
]);

/**
 * @param {string} field
 * @returns {boolean} true when `field` is a known metric field name.
 */
export function isKnownMetric(field) {
  return typeof field === 'string' && KNOWN_METRICS.includes(field);
}

/**
 * Normalize a metrics object: known metrics keep their numeric value;
 * missing or invalid known metrics are marked with UNKNOWN_METRIC; unknown
 * field names are preserved verbatim but explicitly flagged as unknown.
 */
function normalizeMetrics(metrics) {
  const source = metrics && typeof metrics === 'object' ? metrics : {};
  const normalized = {};
  for (const name of KNOWN_METRICS) {
    const value = source[name];
    normalized[name] =
      typeof value === 'number' && Number.isFinite(value) ? value : UNKNOWN_METRIC;
  }
  for (const [name, value] of Object.entries(source)) {
    if (!KNOWN_METRICS.includes(name)) normalized[name] = UNKNOWN_METRIC;
  }
  return normalized;
}

function idOf(value) {
  return typeof value === 'string' && value ? value : null;
}

function textOf(value) {
  return typeof value === 'string' ? value : '';
}

/**
 * Create a provenance record for machine-local history.
 *
 * The factory produces a fully populated, frozen (immutable) record with the
 * schema version, so records persisted to history files are self-describing
 * and append-safe: new records are appended, never rewritten.
 *
 * @param {object} input
 * @param {import('node:crypto').default|object} [input.run] run id, or object {id}
 * @param {import('node:crypto').default|object} [input.session] session id, or object {id}
 * @param {object} [input.repository] repository identity {remote, commit}
 * @param {object} [input.issue] issue/task {issue, task}
 * @param {object} [input.seat] seat {name}
 * @param {object} [input.route] route {name}
 * @param {string} [input.requestedModel]
 * @param {string} [input.servedModel]
 * @param {number|string} [input.startedAt] epoch ms or ISO string
 * @param {number|string} [input.endedAt] epoch ms or ISO string
 * @param {string} [input.outcome] run outcome, e.g. 'succeeded' | 'failed' | 'cancelled'
 * @param {object} [input.tools] tool metadata
 * @param {object} [input.metrics] metric values
 * @param {object} [input.evidence] free-form evidence material
 * @param {number} [now] epoch ms used for createdAt
 * @returns {Readonly<Record<string, unknown>>} frozen provenance record
 */
export function createRecord(input = {}, now = Date.now()) {
  const run = input.run ?? {};
  const session = input.session ?? {};
  const repository = input.repository ?? {};
  const issue = input.issue ?? {};
  const seat = input.seat ?? {};
  const route = input.route ?? {};
  const tools = input.tools ?? {};

  const record = {
    schemaVersion: SCHEMA_VERSION,
    recordType: 'provenance',
    createdAt: now,
    runId: idOf(typeof run === 'object' ? run.id : run) ?? UNKNOWN_METRIC,
    sessionId: idOf(typeof session === 'object' ? session.id : session) ?? UNKNOWN_METRIC,
    repository: {
      remote: textOf(repository.remote),
      commit: textOf(repository.commit),
    },
    issue: {
      issue: textOf(issue.issue),
      task: textOf(issue.task),
    },
    seat: {
      name: textOf(seat.name),
    },
    route: {
      name: textOf(route.name),
    },
    requestedModel: textOf(input.requestedModel),
    servedModel: textOf(input.servedModel),
    startedAt: input.startedAt ?? null,
    endedAt: input.endedAt ?? null,
    outcome: textOf(input.outcome),
    tools: {
      name: textOf(tools.name),
      version: textOf(tools.version),
    },
    metrics: Object.freeze(normalizeMetrics(input.metrics)),
    evidence: Object.freeze(
      input.evidence && typeof input.evidence === 'object' ? input.evidence : {},
    ),
  };
  return Object.freeze(record);
}
