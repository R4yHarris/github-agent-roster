// Deterministic JSON export and human-readable summary for durable provenance
// history. Exports are self-contained: they carry a schema version, the
// machine-history store root, and stable record ids, with no external state
// required to interpret them.
import { historyFields } from './history-query.mjs';
import { STATE_SCOPES } from './paths.mjs';
import {
  createRedactionConfig,
  redactRecord,
  validateProvenance,
} from './history-redaction.mjs';

export const HISTORY_EXPORT_SCHEMA_VERSION = 1;
const EXPORT_SCHEMA = `roster.history-export.v${HISTORY_EXPORT_SCHEMA_VERSION}`;

const SUMMARY_KEYS = ['repository', 'issue', 'seat', 'model', 'outcome', 'at'];

// Every exported record must carry verified provenance. Validation fails
// closed: a missing, malformed, version-mismatched, or tampered provenance
// block rejects the export before any redacted bytes are produced.
function requireValidProvenance(records) {
  if (!Array.isArray(records)) throw new TypeError('exportHistory requires an array of provenance records.');
  for (let i = 0; i < records.length; i += 1) {
    const result = validateProvenance(records[i]);
    if (!result.ok) throw new Error(`Provenance validation failed for record ${i}: ${result.error}`);
  }
}

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
    integrity: validateProvenance(record).integrity ?? 'invalid',
  };
  if (record.repoIdentity !== undefined) entry.repository = record.repoIdentity;
  // Configuration-driven redaction: explicit secret values/paths, home
  // masking with per-record opt-in, and deterministic [REDACTED] markers.
  return redactRecord(entry, createRedactionConfig());
}

// Deterministic JSON export of equivalent input: schema version, stable
// record ids, sorted object keys, and original record order preserved.
export function exportHistory(records, meta = {}) {
  requireValidProvenance(records);
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


// ---------------------------------------------------------------------------
// Scoped deletion (issue #201): explicit, scoped, previewable.
//
// Deletion is a pure function of its inputs: the record list is never mutated,
// only the selected scope's records are reported as deleted, and the audit
// entry carries ids and counts (plus redacted summaries), never raw secret
// values. Expiring one scope leaves every other scope untouched.
// ---------------------------------------------------------------------------

const DELETION_AUDIT_SCHEMA = 'roster.history-deletion-audit.v1';

function normalizeDeletionScope(scope) {
  if (typeof scope !== 'string' || !STATE_SCOPES.includes(scope)) {
    throw new TypeError(`Scoped deletion requires a scope in ${JSON.stringify(STATE_SCOPES)}; got ${JSON.stringify(scope)}.`);
  }
  return scope;
}

function recordScope(record) {
  if (record === null || typeof record !== 'object' || Array.isArray(record)) return undefined;
  const scope = record.scope ?? record.stateScope;
  return typeof scope === 'string' && STATE_SCOPES.includes(scope) ? scope : undefined;
}

// Builds the small, redaction-safe summary an audit entry may carry for a
// deleted record. Ids, run/session, and stable history fields are safe; the
// full payload never enters the audit trail. Any secret-looking value is
// replaced with the shared redaction marker (reusing the export redaction).
function auditSummaryFor(record) {
  // canonicalizeRecord already passes through redactRecord (export redaction);
  // redactSecrets covers the key-name and credential-literal rules, so the
  // audit summary is double-redacted and can never carry raw secret values.
  const entry = canonicalizeRecord(record);
  const summary = {
    ...(entry.id !== undefined ? { id: entry.id } : {}),
    ...(entry.runId !== undefined ? { runId: entry.runId } : {}),
    ...(entry.sessionId !== undefined ? { sessionId: entry.sessionId } : {}),
    fields: redactSecrets(entry.fields),
  };
  return redactSecrets(summary);
}

// Preview a scoped deletion: returns the record ids and scopes that would be
// deleted, without mutating the input records. Scope isolation is exact: only
// records whose scope matches the selector are listed.
export function previewScopedDeletion(records, { scope } = {}) {
  if (!Array.isArray(records)) throw new TypeError('previewScopedDeletion requires an array of history records.');
  const resolved = normalizeDeletionScope(scope);
  const matching = records.filter((record) => recordScope(record) === resolved);
  return {
    scope: resolved,
    count: matching.length,
    records: matching.map((record) => {
      const summary = auditSummaryFor(record);
      return { id: summary.id, scope: resolved, summary };
    }),
  };
}

// Applies a scoped deletion: returns the surviving records and an audit entry.
// The input array and every record object are left untouched; records in
// other scopes survive byte-for-byte. The audit records the scope, deleted
// record ids, and a count, and references secrets only in redacted form.
export function deleteScopedRecords(records, { scope, now = () => new Date().toISOString() } = {}) {
  if (!Array.isArray(records)) throw new TypeError('deleteScopedRecords requires an array of history records.');
  const preview = previewScopedDeletion(records, { scope });
  const remaining = records.filter((record) => recordScope(record) !== preview.scope);
  const audit = {
    schema: DELETION_AUDIT_SCHEMA,
    scope: preview.scope,
    at: now(),
    count: preview.count,
    recordIds: preview.records.map((entry) => entry.id),
    records: preview.records.map((entry) => ({ id: entry.id, scope: entry.scope, summary: entry.summary })),
  };
  return { remaining, audit };
}

export function renderDeletionAudit(audit) {
  if (audit === null || typeof audit !== 'object' || Array.isArray(audit)) {
    throw new TypeError('renderDeletionAudit requires an audit object from deleteScopedRecords.');
  }
  const lines = [
    `Deletion audit: scope=${audit.scope} deleted=${audit.count}`,
    ...(Array.isArray(audit.recordIds) ? audit.recordIds.map((id) => `- ${id}`) : []),
  ];
  if (audit.at !== undefined) lines.push(`at=${audit.at}`);
  return `${lines.join('\n')}\n`;
}

// Human-readable summary that clearly distinguishes the machine-history
// location (the provenance store root) from repository-state paths.
export function renderHistorySummary(records, meta = {}) {
  requireValidProvenance(records);
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
