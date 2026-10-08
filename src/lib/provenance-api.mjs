import { createHash } from 'node:crypto';
import path from 'node:path';
import { compareIdentity, resolveRepoIdentity } from './repo-identity.mjs';
import { SCHEMA_VERSION, createRecord } from './provenance-schema.mjs';
import { openProvenanceStore, ProvenanceStoreError, validateProvenanceData,
  validateProvenanceRecord as earlierValidate } from './provenance-store.mjs';
import { redactRecord } from './redaction.mjs';

// Re-exports keep the earlier-wave names on this module's public surface.
export { openProvenanceStore } from './provenance-store.mjs';
export { SCHEMA_VERSION } from './provenance-schema.mjs';

const RUN_LIFECYCLE_EVENTS = ['started', 'session', 'failure', 'cancellation', 'completed'];
const RAW_SECTION = 'raw-history';
const CURATED_SECTION = 'curated-memory';
const COMPACTION_SECTION = 'compaction';
// Matches the event baked into compaction record ids by `buildCompactionRecord`
// so provenance and ids never drift apart.
const COMPACTION_EVENT = 'compacted';

const isNonEmptyString = (value) => typeof value === 'string' && value.trim() !== '';

function validatePayload(payload) {
  if (payload === null || typeof payload !== 'object' || Array.isArray(payload)) {
    throw new TypeError('provenance payload must be a plain JSON object');
  }
  validateProvenanceData(payload, 'payload');
  return payload;
}

/**
 * Typed provenance record validation. A record must carry a run id, session
 * id, and one of the five required lifecycle events, plus a stable repository
 * identity (algorithm-hex) when present. The earlier-wave
 * provenance-store.mjs#validateProvenanceRecord is run as a secondary check
 * so the two validation stacks agree.
 */
export function validateProvenanceRecord(record, source = 'provenance record') {
  if (typeof record !== 'object' || record === null || Array.isArray(record)) {
    throw new TypeError(`${source} must be an object`);
  }
  const stringField = (name) => {
    const value = record[name];
    if (!isNonEmptyString(value)) {
      throw new TypeError(`${source} ${name} must be a non-empty string`);
    }
    return value;
  };
  const runId = stringField('runId');
  const sessionId = stringField('sessionId');
  const event = stringField('event');
  if (!RUN_LIFECYCLE_EVENTS.includes(event)) {
    throw new TypeError(`${source} event must be one of ${RUN_LIFECYCLE_EVENTS.join(', ')}`);
  }
  const repoIdentity = record.repoIdentity;
  if (repoIdentity !== undefined && !/^[a-z0-9]+-[a-f0-9]{64}$/.test(repoIdentity)) {
    throw new TypeError(`${source} repoIdentity must be a derived identity hash (algorithm-hex)`);
  }
  const payload = validatePayload(record.payload === undefined ? {} : record.payload);
  if (typeof earlierValidate === 'function') {
    const result = earlierValidate(
      { id: record.id ?? `${runId}/${sessionId}`, version: record.version ?? 1, ...record },
      source,
    );
    if (result && result.ok === false) {
      throw new TypeError(`${source} failed earlier-wave validation: ${result.reason ?? 'unknown'}`);
    }
  }
  return {
    ...(repoIdentity === undefined ? {} : { repoIdentity }),
    runId, sessionId, event, payload,
  };
}

/**
 * Build a typed provenance record by delegating field defaults and schema
 * stamping to the earlier-wave `createRecord`. The typed API adds the stable
 * repository identity and redacts any secret-looking material.
 */
export function buildProvenanceRecord({ runId, sessionId, event, payload } = {}, { repoIdentity, now = Date.now(), redact = true } = {}) {
  const payloadObject = validatePayload(payload === undefined ? {} : payload);
  const record = createRecord({
    runId,
    sessionId,
    event,
    ...(payloadObject ? { payload: payloadObject } : {}),
  }, now);
  const normalized = {
    ...record,
    runId,
    sessionId,
    event,
    ...(payloadObject ? { payload: payloadObject } : { payload: {} }),
    ...(isNonEmptyString(repoIdentity) ? { repoIdentity } : {}),
  };
  return redact ? redactRecord(normalized) : normalized;
}

/**
 * Derive a safe, unique record id for the earlier-wave durable store.
 * The id must consist only of [A-Za-z0-9._-] characters (≤ 256 chars) so it
 * survives the store's file-name sanitization unchanged.
 */
export function storeRecordId(identity, runId, sessionId, event, section) {
  const raw = `${identity}|${runId}|${sessionId}|${event}|${section}`;
  return createHash('sha256').update(raw).digest('hex');
}

/**
 * Build compaction provenance for a single source memory record.
 *
 * Compaction re-records surviving memory records under a stable compaction
 * identity so curated memory can link back to source record ids. Each source
 * record yields:
 *   - a `sourceRecordId` for the original record (stable across runs),
 *   - a provenance record (via `buildProvenanceRecord`) stamped with the
 *     source run/session ids so provenance survives compaction.
 *
 * Missing provenance fields fall back to the record's own `runId`/
 * `sessionId` rather than throwing. `validateProvenanceRecord` is applied
 * when the event is a lifecycle event; the compaction payload itself is
 * always validated for shape.
 */
export function buildCompactionProvenance(sourceRecord, { identity = 'compaction', source = 'compaction source' } = {}) {
  if (typeof sourceRecord !== 'object' || sourceRecord === null || Array.isArray(sourceRecord)) {
    throw new TypeError(`${source} must be an object`);
  }
  const runId = isNonEmptyString(sourceRecord.runId) ? sourceRecord.runId : 'unknown';
  const sessionId = isNonEmptyString(sourceRecord.sessionId) ? sourceRecord.sessionId : 'unknown';
  // A missing event must not be fabricated as a lifecycle event: fall back to
  // the compaction event itself so provenance never claims a run state that
  // the source record did not report.
  const event = isNonEmptyString(sourceRecord.event) ? sourceRecord.event : COMPACTION_EVENT;
  // Same-run records share run, session, and event; the content digest keeps their ids distinct.
  const digest = createHash('sha256').update(JSON.stringify(sourceRecord)).digest('hex');
  const sourceRecordId = storeRecordId(identity, runId, sessionId, `${event}:${digest}`, RAW_SECTION);
  const payload = {
    ...sourceRecord,
    runId,
    sessionId,
    // Preserve provenance already merged by an earlier compaction pass
    // instead of overwriting it with this record's own ids.
    sourceRunIds: [...new Set([runId, ...(sourceRecord.sourceRunIds ?? [])])],
    sourceSessionIds: [...new Set([sessionId, ...(sourceRecord.sourceSessionIds ?? [])])],
    sourceRecordId,
  };
  let provenance;
  try {
    provenance = buildProvenanceRecord({ runId, sessionId, event, payload });
  } catch (error) {
    // Only the missing-field fallback is tolerated; construction failures for
    // otherwise-valid records must surface to the caller.
    if (!(error instanceof TypeError)) throw error;
    provenance = redactRecord({ runId, sessionId, event, payload });
  }
  // Lifecycle-event provenance is validated with the typed validator; missing
  // provenance fields fall back to the record's own ids (TASK.md edge case)
  // before validating, and every other validation failure surfaces.
  if (RUN_LIFECYCLE_EVENTS.includes(event)) {
    if (!isNonEmptyString(provenance.runId) || !isNonEmptyString(provenance.sessionId)) {
      provenance.runId = runId;
      provenance.sessionId = sessionId;
    }
    validateProvenanceRecord(provenance, 'compaction provenance');
  }
  return { sourceRecordId, provenance };
}

/**
 * Build a compaction record from a source memory record. The compaction
 * record carries the source run/session ids plus a `sourceRecordId` (the
 * stable id of the source record) so curated memory can link back.
 */
export function buildCompactionRecord(sourceRecord, options = {}) {
  const { identity = 'compaction' } = options;
  const { sourceRecordId, provenance } = buildCompactionProvenance(sourceRecord, options);
  const compactionRecordId = storeRecordId(identity, provenance.runId, provenance.sessionId,
    `${COMPACTION_EVENT}:${sourceRecordId}`, COMPACTION_SECTION);
  return {
    id: compactionRecordId,
    section: COMPACTION_SECTION,
    runId: provenance.runId,
    sessionId: provenance.sessionId,
    event: provenance.event,
    sourceRunIds: [...new Set([provenance.runId, ...(sourceRecord.sourceRunIds ?? [])])],
    sourceSessionIds: [...new Set([provenance.sessionId, ...(sourceRecord.sourceSessionIds ?? [])])],
    sourceRecordId,
    provenance,
    payload: provenance.payload,
  };
}

function validateMemoryRecord(record, source = 'curated memory record') {
  if (typeof record !== 'object' || record === null || Array.isArray(record)) {
    throw new TypeError(`${source} must be an object`);
  }
  if (typeof record.repoIdentity !== 'string' || record.repoIdentity.trim() === '') {
    throw new TypeError(`${source} repoIdentity must be a non-empty string`);
  }
  if (typeof record.runId !== 'string' || record.runId.trim() === '') {
    throw new TypeError(`${source} runId must be a non-empty string`);
  }
  if (typeof record.sessionId !== 'string' || record.sessionId.trim() === '') {
    throw new TypeError(`${source} sessionId must be a non-empty string`);
  }
  if (typeof record.memory !== 'string' || record.memory.trim() === '') {
    throw new TypeError(`${source} memory must be a non-empty string`);
  }
  return record;
}

function matches(record, { runId, sessionId, event }) {
  if (runId !== undefined && record.runId !== runId) return false;
  if (sessionId !== undefined && record.sessionId !== sessionId) return false;
  if (event !== undefined && record.event !== event) return false;
  return true;
}

/**
 * ProvenanceStore: the only way to read or write provenance records.
 *
 * This class is a typed facade over the earlier-wave durable store
 * (provenance-store.mjs#openProvenanceStore). Every durable read/write flows
 * through the earlier-wave store's `appendRecord` / `readAll`, so there is
 * one storage stack, not two. Records are namespaced by repository identity
 * (a hash string that cannot be a directory name) so two repositories cannot
 * collide or leak. Raw history and curated memory are separated by a
 * `section` field on each record.
 */
export class ProvenanceStore {
  constructor({
    root,
    resolveIdentity = () => { throw new Error('resolveIdentity is required'); },
    underlying,
  } = {}) {
    if (typeof root !== 'string' || root.trim() === '') {
      throw new TypeError('ProvenanceStore requires a root path');
    }
    this.root = path.resolve(root);
    this.resolveIdentity = resolveIdentity;
    // Always use the earlier-wave durable store for I/O. If the caller
    // already opened one, use it; otherwise open a fresh one rooted here.
    this.underlying = underlying ?? openProvenanceStore(this.root);
  }

  async #identity() {
    const identity = await this.resolveIdentity();
    if (typeof identity !== 'string' || identity.trim() === '') {
      throw new TypeError('resolveIdentity must resolve to a non-empty identity hash');
    }
    return identity;
  }

  #seq = 0;

  async #persist(record) {
    record.seq = this.#seq++;
    await this.underlying.appendRecord(record);
    return record;
  }

  async #load(identity, section) {
    const { records, skipped = [] } = await this.underlying.readAll();
    if (skipped.length) {
      throw new ProvenanceStoreError(
        `Provenance contains ${skipped.length} malformed or incompatible record(s); run store.repair() to quarantine them before querying.`,
        { code: 'E_INVALID_RECORD', path: this.root });
    }
    return records
      .filter((record) => {
        if (!record || typeof record !== 'object') return false;
        if (record.repoIdentity !== identity) return false;
        if (record.section !== section) return false;
        return true;
      })
      .sort((a, b) => (a.seq ?? 0) - (b.seq ?? 0));
  }

  /**
   * Append a typed provenance record to raw history.
   */
  async record({ runId, sessionId, event, payload } = {}) {
    const identity = await this.#identity();
    const record = buildProvenanceRecord({ runId, sessionId, event, payload }, { repoIdentity: identity });
    validateProvenanceRecord(record, 'provenance record');
    record.id = storeRecordId(identity, runId, sessionId, event, RAW_SECTION);
    record.version = 1;
    record.section = RAW_SECTION;
    await this.#persist(record);
    return record;
  }

  /**
   * Record a run/session lifecycle event.
   */
  async recordEvent({ runId, sessionId, event, payload } = {}) {
    return this.record({ runId, sessionId, event, payload });
  }

  /**
   * Append curated memory. Separated from raw history by design.
   */
  async recordMemory({ runId, sessionId, memory, payload } = {}) {
    const identity = await this.#identity();
    if (typeof memory !== 'string' || memory.trim() === '') {
      throw new TypeError('curated memory requires a non-empty memory string');
    }
    if (payload !== undefined) validatePayload(payload);
    const record = redactRecord({
      repoIdentity: identity,
      runId,
      sessionId,
      event: 'memory',
      memory,
      ...(payload !== undefined ? { payload } : {}),
    });
    validateMemoryRecord(record, 'curated memory record');
    record.id = storeRecordId(identity, runId, sessionId, 'memory', CURATED_SECTION);
    record.version = 1;
    record.section = CURATED_SECTION;
    await this.#persist(record);
    return record;
  }

  /**
   * Query records through the typed API. The only supported way for
   * routing/eval readers to consume provenance.
   */
  async query({ repoIdentity, runId, sessionId, event, section = RAW_SECTION } = {}) {
    const identity = await this.#identity();
    if (repoIdentity !== undefined && repoIdentity !== identity) {
      return [];
    }
    if (section === CURATED_SECTION) {
      const records = await this.#load(identity, CURATED_SECTION);
      return records.filter((record) => matches(record, { runId, sessionId, event }));
    }
    if (section !== RAW_SECTION) {
      throw new TypeError('section must be "raw-history" or "curated-memory"');
    }
    const records = await this.#load(identity, RAW_SECTION);
    return records.filter((record) => matches(record, { runId, sessionId, event }));
  }

  /**
   * Resolve the store's repository identity.
   */
  async identity() {
    return this.#identity();
  }
}

/**
 * Factory: builds a ProvenanceStore with identity resolution from
 * repo-identity.mjs. Always opens an earlier-wave durable store for I/O.
 */
export function createProvenanceStore({
  root,
  repoRoot = process.cwd(),
  run,
  underlying,
} = {}) {
  const resolveIdentity = run === undefined
    ? () => resolveRepoIdentity({ repoRoot })
    : () => resolveRepoIdentity({ repoRoot, run });
  const resolved = underlying ?? openProvenanceStore(root);
  return new ProvenanceStore({
    root,
    resolveIdentity,
    underlying: resolved,
  });
}

/**
 * Integration hook: wraps a run/session lifecycle and captures every event
 * through the typed API. When `optOut` is true the hooks still fire but no
 * durable record is written.
 */
export function instrumentLifecycle(store, { optOut = false } = {}) {
  const capture = async ({ runId, sessionId, event, payload }) => {
    if (optOut) return { runId, sessionId, event, durable: false };
    const record = await store.recordEvent({ runId, sessionId, event, payload });
    return { ...record, durable: true };
  };
  return {
    started: (ctx) => capture({ ...ctx, event: 'started' }),
    session: (ctx) => capture({ ...ctx, event: 'session' }),
    failure: (ctx) => capture({ ...ctx, event: 'failure' }),
    cancellation: (ctx) => capture({ ...ctx, event: 'cancellation' }),
    completed: (ctx) => capture({ ...ctx, event: 'completed' }),
  };
}

/**
 * Routing/eval reader facade: the only supported path for these readers to
 * consume provenance. Exposes query surfaces and nothing filesystem-level.
 */
export function createRoutingReader(store) {
  return {
    byEvent(event, { runId, sessionId, section } = {}) {
      return store.query({ event, runId, sessionId, section });
    },
    run(runId, { section } = {}) {
      return store.query({ runId, section });
    },
    session(sessionId, { section } = {}) {
      return store.query({ sessionId, section });
    },
    rawHistory({ runId, sessionId, event } = {}) {
      return store.query({ runId, sessionId, event, section: 'raw-history' });
    },
    curatedMemory({ runId, sessionId } = {}) {
      return store.query({ runId, sessionId, section: 'curated-memory' });
    },
  };
}

export { compareIdentity };
