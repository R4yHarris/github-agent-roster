// Redaction and provenance-integrity helpers for durable history exports.
//
// Redaction is explicit and configuration-driven: only values named by the
// configuration are masked, and masking uses a single deterministic marker.
// Unknown, missing, or null metrics are never synthesized into values.
// Provenance validation verifies version and integrity (hash/checksum or
// signature) before export and fails closed on any mismatch.
import { createHash } from 'node:crypto';
import { storeRecordId } from './provenance-api.mjs';
import { validateProvenanceRecord as validateStoredRecord } from './provenance-store.mjs';
import { redactEvidence } from './redaction.mjs';

const TYPED_SECTIONS = { 'raw-history': (record) => record.event, 'curated-memory': () => 'memory' };

// Typed-API records (every live run record) derive their id from repo identity,
// run, session, event, and section, so recomputing it detects tampering with
// any of those fields without a separate checksum.
function verifyStoredIdentity(record) {
  const stored = validateStoredRecord(record);
  if (!stored.ok) return { ok: false, error: `stored record is invalid: ${stored.reason}` };
  if (record.section === undefined) return { ok: true, integrity: 'unbound' };
  const event = TYPED_SECTIONS[record.section];
  if (!event) return { ok: false, error: `unknown provenance section ${JSON.stringify(record.section)}` };
  const expected = storeRecordId(record.repoIdentity, record.runId, record.sessionId, event(record), record.section);
  if (record.id !== expected) return { ok: false, error: 'record id does not match its derived provenance identity' };
  return { ok: true, integrity: 'verified' };
}

export const REDACTION_MARKER = '[REDACTED]';
export const HOME_MARKER = '~';
export const PROVENANCE_SCHEMA = 'roster.history-provenance';
export const PROVENANCE_SCHEMA_VERSION = 1;
const PROVENANCE_ALGORITHM = 'sha256';

// ---------------------------------------------------------------------------
// Redaction configuration
// ---------------------------------------------------------------------------

// Builds the default redaction configuration. Environment names listed in
// `secretEnvNames` (default: API_KEY, PRIVATE_KEY_PATH) contribute their
// current values to the masked-value set. Values must be supplied explicitly;
// nothing is inferred from record contents.
export function createRedactionConfig(options = {}) {
  const env = options.env ?? process.env;
  const config = {
    values: [],
    paths: [],
    home: null,
    keepHome: false,
  };
  if (options.values) for (const value of options.values) addConfigValue(config, value);
  if (options.paths) for (const path of options.paths) addConfigPath(config, path);
  if (options.home !== undefined) config.home = String(options.home);
  if (options.keepHome) config.keepHome = true;
  const envNames = options.secretEnvNames ?? ['API_KEY', 'PRIVATE_KEY_PATH'];
  for (const name of envNames) {
    const value = env[name];
    if (typeof value === 'string' && value) addConfigValue(config, value);
  }
  return config;
}

function addConfigValue(config, value) {
  if (typeof value === 'string' && value) config.values.push(value);
}

function addConfigPath(config, path) {
  if (typeof path === 'string' && path) config.paths.push(path);
}

// ---------------------------------------------------------------------------
// String and record redaction
// ---------------------------------------------------------------------------

// Replaces every configured secret value and private key path in a string.
// Values are applied longest-first so overlapping configured values cannot
// leave fragments behind. Unknown text is returned unchanged.
export function redactString(value, config = {}) {
  if (typeof value !== 'string') return value;
  let out = value;
  if (config.values) {
    for (const secret of [...config.values].sort((a, b) => b.length - a.length)) {
      if (secret) out = out.split(secret).join(REDACTION_MARKER);
    }
  }
  if (config.paths) {
    for (const path of [...config.paths].sort((a, b) => b.length - a.length)) {
      if (path) out = out.split(path).join(REDACTION_MARKER);
    }
  }
  return out;
}

// Replaces an absolute home-directory prefix with `~` (or `[HOME]`) unless
// the record explicitly opts in to keep the required detail (keepHome or a
// top-level keepHome flag on the record itself).
function maskHome(value, config) {
  if (typeof value !== 'string' || !config || !config.home) return value;
  if (config.keepHome || value === config.home) return value;
  if (value.startsWith(`${config.home}/`) || value.startsWith(`${config.home}\\`)) {
    return `${HOME_MARKER}${value.slice(config.home.length)}`;
  }
  return value;
}

// Redacts one history record: configured secret values/paths everywhere, and
// home-directory details unless the record opts in to keep them.
//
// String leaves additionally pass through the shared evidence redaction
// (redactEvidence) so that credential-shaped material and secret-looking
// assignments are masked even when the configuration does not name them
// explicitly. This keeps redaction fail-closed: a record can never carry a
// live secret into persistence merely because the config missed a value.
export function redactRecord(record, config = {}) {
  if (record === null || typeof record !== 'object') return record;
  const perRecord = { ...config, keepHome: record.keepHome ? true : config.keepHome };
  return redactValue(record, perRecord);
}

function redactValue(value, config) {
  if (typeof value === 'string') {
    const configured = maskHome(redactString(value, config), config);
    return redactEvidence(configured, { env: config.env });
  }
  if (Array.isArray(value)) return value.map((item) => redactValue(item, config));
  if (value !== null && typeof value === 'object') {
    const out = {};
    for (const [key, item] of Object.entries(value)) {
      out[key] = redactValue(item, config);
    }
    return out;
  }
  return value;
}

// ---------------------------------------------------------------------------
// Provenance integrity
// ---------------------------------------------------------------------------

// Canonical form of a provenance record for hashing: the record's stable
// identity fields and export fields. The provenance block itself, and any
// redaction/home opt-in flag, are excluded so masking never changes the hash.
function provenanceDigestSource(record) {
  return {
    id: record.id ?? null,
    runId: record.runId ?? null,
    sessionId: record.sessionId ?? null,
    repoIdentity: record.repoIdentity ?? null,
    issue: record.issue ?? null,
    seat: record.seat ?? null,
    servedModel: record.servedModel ?? null,
    requestedModel: record.requestedModel ?? null,
    payload: record.payload ?? null,
    outcome: record.outcome ?? null,
    event: record.event ?? null,
    createdAt: record.createdAt ?? null,
  };
}

// Deterministic JSON encoding used for digest computation (sorted keys).
export function canonicalProvenanceJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalProvenanceJson).join(',')}]`;
  if (value !== null && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalProvenanceJson(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

// Computes the expected sha256 digest for a provenance record.
export function computeProvenanceDigest(record) {
  return createHash('sha256').update(canonicalProvenanceJson(provenanceDigestSource(record))).digest('hex');
}

// Creates a complete provenance block (schema, version, algorithm, digest)
// for a record. Used by producers and tests to mint valid provenance.
export function createProvenance(record, overrides = {}) {
  const base = {
    schema: PROVENANCE_SCHEMA,
    version: PROVENANCE_SCHEMA_VERSION,
    algorithm: PROVENANCE_ALGORITHM,
    digest: computeProvenanceDigest(record),
  };
  return { ...base, ...overrides };
}

// Validates one record's provenance. Returns `{ ok: true }` when the block
// verifies, or `{ ok: false, error }` when it is missing, malformed, or
// tampered. Verification is fail-closed: any check that cannot pass fails.
export function validateProvenance(record) {
  if (record === null || typeof record !== 'object' || Array.isArray(record)) {
    return { ok: false, error: 'provenance record is not an object' };
  }
  const identity = verifyStoredIdentity(record);
  if (!identity.ok) return identity;
  // An optional provenance block adds a digest over the exported fields.
  // Once present but not an object (null, string, array), it is malformed and fails.
  if (!('provenance' in record) || record.provenance === undefined) return identity;
  const block = record.provenance;
  if (block === null || typeof block !== 'object' || Array.isArray(block)) {
    return { ok: false, error: 'provenance block is malformed' };
  }
  if (block.schema !== PROVENANCE_SCHEMA) {
    return { ok: false, error: `provenance schema must be ${PROVENANCE_SCHEMA}` };
  }
  if (block.version !== PROVENANCE_SCHEMA_VERSION) {
    return { ok: false, error: `provenance version must be ${PROVENANCE_SCHEMA_VERSION}` };
  }
  const expected = computeProvenanceDigest(record);
  const verified =
    (typeof block.digest === 'string' && block.digest.length > 0 && block.digest === expected) ||
    (typeof block.hash === 'string' && block.hash.length > 0 && block.hash === expected) ||
    (typeof block.checksum === 'string' && block.checksum.length > 0 && block.checksum === expected) ||
    (typeof block.signature === 'string' && block.signature.length > 0 && block.signature === expected);
  if (!verified) {
    return { ok: false, error: 'provenance integrity check failed' };
  }
  return { ok: true, integrity: 'verified' };
}

// Verifies every record and quarantines the schema-incompatible ones.
//
// Fail-closed semantics: the result is `ok: false` whenever any record fails,
// and the first offender is exposed via `index` / `error` so callers can fail
// the export. Every offender is listed in `quarantine` (`{ index, error }`) so
// a migration tool can set each bad record aside; valid sibling records are
// never dropped or mutated — they simply validate successfully. The input
// array is not modified.
export function validateProvenanceRecords(records) {
  if (!Array.isArray(records)) {
    return { ok: false, error: 'provenance records must be an array' };
  }
  const quarantine = [];
  for (let i = 0; i < records.length; i += 1) {
    const result = validateProvenance(records[i]);
    if (!result.ok) quarantine.push({ index: i, error: result.error });
  }
  if (quarantine.length) {
    return { ok: false, index: quarantine[0].index, error: quarantine[0].error, quarantine };
  }
  return { ok: true, quarantine };
}

// ---------------------------------------------------------------------------
// Migration scan
// ---------------------------------------------------------------------------

// Required stable fields for a legacy history record (id/version come from
// the provenance store contract; runId/sessionId from the typed API).
const REQUIRED_MIGRATION_FIELDS = ['id', 'version', 'runId', 'sessionId'];

// Scans a legacy record array for malformed entries and returns the indices
// of the offenders, in order. A record is malformed when it is not a plain
// object, is missing any required field, or fails the provenance store's
// record-shape validation (`validateProvenanceRecord` style: non-empty safe
// id string, current record version). Non-object entries, missing required
// fields, and shape-invalid records are all reported; valid entries are not.
export function scanMalformedRecords(records) {
  if (!Array.isArray(records)) throw new TypeError('legacy history records must be an array');
  const indices = [];
  for (let i = 0; i < records.length; i += 1) {
    const record = records[i];
    if (record === null || typeof record !== 'object' || Array.isArray(record)) {
      indices.push(i);
      continue;
    }
    const missing = REQUIRED_MIGRATION_FIELDS.some(
      (field) => record[field] === undefined || record[field] === null,
    );
    if (missing) {
      indices.push(i);
      continue;
    }
    const verdict = validateStoredRecord(record);
    if (!verdict.ok) indices.push(i);
  }
  return indices;
}
