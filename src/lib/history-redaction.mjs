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

function redactValue(value, config) {
  if (typeof value === 'string') return maskHome(redactString(value, config), config);
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

// Redacts one history record: configured secret values/paths everywhere, and
// home-directory details unless the record opts in to keep them.
export function redactRecord(record, config = {}) {
  if (record === null || typeof record !== 'object') return record;
  const perRecord = { ...config, keepHome: record.keepHome ? true : config.keepHome };
  return redactValue(record, perRecord);
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

// Verifies every record up front, returning the first failure. Exporters use
// this to fail closed before any redacted bytes are produced.
export function validateProvenanceRecords(records) {
  if (!Array.isArray(records)) {
    return { ok: false, error: 'provenance records must be an array' };
  }
  for (let i = 0; i < records.length; i += 1) {
    const result = validateProvenance(records[i]);
    if (!result.ok) return { ok: false, index: i, error: result.error };
  }
  return { ok: true };
}
