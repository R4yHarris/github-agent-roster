// Read-only queries over durable provenance (FEATURE_SPEC sections 5.6–5.8).
import { openProvenanceStore } from './provenance-store.mjs';

const FILTER_KEYS = new Set(['repository', 'issue', 'seat', 'model', 'outcome', 'since', 'until']);

function first(...values) {
  return values.find((value) => value !== undefined && value !== null && value !== '');
}

export function historyFields(record) {
  return {
    repository: record.repoIdentity,
    issue: first(record.issue?.issue, record.payload?.issue),
    seat: first(record.seat?.name, record.payload?.seat),
    model: first(record.servedModel, record.requestedModel, record.payload?.model),
    outcome: first(record.payload?.outcome, record.outcome, record.event),
    at: record.createdAt,
  };
}

function toMillis(value) {
  if (typeof value === 'number') return Number.isFinite(value) ? value : undefined;
  if (typeof value !== 'string' || !value.trim()) return undefined;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

export function filterRecords(records, filters = {}) {
  for (const key of Object.keys(filters)) {
    if (!FILTER_KEYS.has(key)) throw new TypeError(`Unknown history filter "${key}".`);
  }
  if (!Array.isArray(records)) throw new TypeError('records must be an array of provenance records.');
  const bounds = {};
  for (const key of ['since', 'until']) {
    if (filters[key] === undefined) continue;
    bounds[key] = toMillis(filters[key]);
    if (bounds[key] === undefined) throw new TypeError(`Invalid history ${key} time.`);
  }
  if (bounds.since !== undefined && bounds.until !== undefined && bounds.since > bounds.until) {
    throw new TypeError('history since must not be after until.');
  }
  return records.filter((record) => {
    const fields = historyFields(record);
    for (const key of ['repository', 'issue', 'seat', 'model', 'outcome']) {
      if (filters[key] !== undefined && String(fields[key]) !== String(filters[key])) return false;
    }
    if (bounds.since === undefined && bounds.until === undefined) return true;
    const at = toMillis(fields.at);
    return at !== undefined && (bounds.since === undefined || at >= bounds.since) &&
      (bounds.until === undefined || at <= bounds.until);
  });
}

export function filterRepositories(records, repositories) {
  if (Array.isArray(repositories)) return records.filter((record) => repositories.includes(record.repoIdentity));
  return filterRecords(records, { repository: repositories });
}

export function findHistoryRecord(records, id) {
  if (typeof id !== 'string' || !id.trim()) throw new TypeError('history show requires a session, run, or record id.');
  const match = records.find((record) => [record.runId, record.sessionId, record.id].includes(id));
  if (!match) throw new Error(`No history record found for "${id}".`);
  return match;
}

// Opening is lazy; readAll only reads committed log files. Never call recover,
// repair, appendRecord, or withLock here: those operations mutate the store.
export function createHistoryReader({ root } = {}) {
  if (typeof root !== 'string' || !root.trim()) throw new TypeError('createHistoryReader requires a root path.');
  const store = openProvenanceStore(root);
  return {
    read: () => store.readAll(),
    async list(filters = {}) { return filterRecords((await store.readAll()).records, filters); },
    async show(id) { return findHistoryRecord((await store.readAll()).records, id); },
  };
}
