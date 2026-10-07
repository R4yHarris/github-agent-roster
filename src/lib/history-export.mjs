// Deterministic JSON export and human-readable summary for durable provenance
// history. Exports are self-contained: they carry a schema version, the
// machine-history store root, and stable record ids, with no external state
// required to interpret them.
import { historyFields } from './history-query.mjs';

export const HISTORY_EXPORT_SCHEMA_VERSION = 1;
const EXPORT_SCHEMA = `roster.history-export.v${HISTORY_EXPORT_SCHEMA_VERSION}`;

const SUMMARY_KEYS = ['repository', 'issue', 'seat', 'model', 'outcome', 'at'];

function sortKeys(object) {
  return Object.fromEntries(Object.keys(object).sort().map((key) => [key, object[key]]));
}

function stableStringify(value) {
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(',')}]`;
  if (value !== null && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableStringify(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

// Redaction keeps exports free of credentials: values under secret-looking
// keys are replaced, and secret-looking literals in string values are masked.
const SECRET_KEY_PATTERN = /secret|token|api[_-]?key|password|authorization|credential|bearer/i;
const SECRET_LITERAL = /\b(sk|ghp|gho|github_pat)_[A-Za-z0-9_-]{4,}\b/g;
const PEM_KEY_BLOCK = /-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g;

function redactSecrets(value) {
  if (typeof value === 'string') {
    return value.replace(SECRET_LITERAL, '[redacted]').replace(PEM_KEY_BLOCK, '[redacted]');
  }
  if (Array.isArray(value)) return value.map(redactSecrets);
  if (value !== null && typeof value === 'object') {
    const out = {};
    for (const [key, item] of Object.entries(value)) {
      out[key] = SECRET_KEY_PATTERN.test(key) && item !== undefined ? '[redacted]' : redactSecrets(item);
    }
    return out;
  }
  return value;
}

// Canonicalize one record into a self-contained diagnostic evidence entry.
// Only stable scalar fields are exported (via historyFields), and object key
// ordering is deterministic.
export function canonicalizeRecord(record) {
  if (record === null || typeof record !== 'object' || Array.isArray(record)) {
    throw new TypeError('canonicalizeRecord requires a provenance record object.');
  }
  const fields = historyFields(record);
  const entry = {
    id: record.id,
    runId: record.runId,
    sessionId: record.sessionId,
    fields: sortKeys(Object.fromEntries(SUMMARY_KEYS.map((key) => [key, fields[key] ?? null]))),
  };
  if (record.repoIdentity !== undefined) entry.repository = record.repoIdentity;
  return redactSecrets(entry);
}

// Deterministic JSON export of equivalent input: schema version, stable
// record ids, sorted object keys, and original record order preserved.
export function exportHistory(records, meta = {}) {
  if (!Array.isArray(records)) throw new TypeError('exportHistory requires an array of provenance records.');
  return redactSecrets({
    meta: sortKeys({
      format: EXPORT_SCHEMA,
      schemaVersion: HISTORY_EXPORT_SCHEMA_VERSION,
      // Timestamps and skip counts appear only when supplied so equal input exports byte-identically.
      ...(meta.exportedAt === undefined ? {} : { exportedAt: meta.exportedAt }),
      ...(meta.skippedCount === undefined ? {} : { skippedCount: meta.skippedCount }),
      storeRoot: meta.storeRoot ?? null,
      recordCount: records.length,
    }),
    records: records.map(canonicalizeRecord),
  });
}

export function renderHistoryJson(records, meta = {}) {
  return `${stableStringify(sortKeys(exportHistory(records, meta)))}\n`;
}

// Human-readable summary that clearly distinguishes the machine-history
// location (the provenance store root) from repository-state paths.
export function renderHistorySummary(records, meta = {}) {
  if (!Array.isArray(records)) throw new TypeError('renderHistorySummary requires an array of provenance records.');
  const lines = [`History summary: ${records.length} record(s)`];
  if (meta.storeRoot !== undefined && meta.storeRoot !== null) {
    lines.push(`Machine history store: ${meta.storeRoot}`);
  }
  for (const record of records) {
    const entry = canonicalizeRecord(record);
    const { fields, repository } = entry;
    lines.push(`- ${entry.id ?? '-'}  run=${entry.runId ?? '-'}  session=${entry.sessionId ?? '-'}`);
    lines.push(`    repo=${repository ?? fields.repository ?? '-'}`);
    lines.push(`    issue=${fields.issue ?? '-'}  seat=${fields.seat ?? '-'}  model=${fields.model ?? '-'}  outcome=${fields.outcome ?? '-'}  at=${fields.at ?? '-'}`);
  }
  if (meta.repoState !== undefined && meta.repoState !== null) {
    lines.push(`Repository state (not part of machine history): ${meta.repoState}`);
  }
  return `${lines.join('\n')}\n`;
}
