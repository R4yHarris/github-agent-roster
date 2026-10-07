import { promises as fs } from 'node:fs';
import { join, dirname, basename } from 'node:path';
import { randomBytes } from 'node:crypto';
import { acquireRepoLock, RepoLockError } from './repo-locks.mjs';

/**
 * Atomic provenance persistence with crash recovery (parent #195).
 *
 * Layout under the store root:
 *
 *   <root>/log/<id>.json          committed provenance records (append-safe)
 *   <root>/log/.<id>.<nonce>.tmp  interrupted atomic write (recovered)
 *   <root>/quarantine/            malformed or incompatible records
 *
 * Every record write is: exclusive create of a temp file in the same
 * directory, write, fsync, close, atomic rename over the target, then
 * fsync the directory — all while holding the store lock. An interrupt at
 * any point leaves either nothing (pre-rename) or the complete record
 * (post-rename); the temp file is the crash marker, and `recover()` reports
 * such writes as incomplete and removes them, so a retry of the same record
 * id is idempotent and never accepted twice.
 *
 * Records are one JSON object per line in an append-only segment file
 * (`<root>/provenance.log`); each line is fsynced before the lock is
 * released. Malformed or incompatible lines are detected at read time,
 * quarantined (moved out of the log via an atomic rebuild), and excluded
 * from normal reads without truncating or corrupting valid records.
 */

export const LOCK_NAME = 'provenance-store';
export const LOG_DIRNAME = 'log';
export const QUARANTINE_DIRNAME = 'quarantine';
export const SEGMENT_NAME = 'provenance.log';
export const SEGMENT_SUFFIX = '.tmp';

export const RECORD_VERSION = 1;

export class ProvenanceStoreError extends Error {
  constructor(message, { code, path, recordId, cause } = {}) {
    super(message);
    this.name = 'ProvenanceStoreError';
    if (code !== undefined) this.code = code;
    if (path !== undefined) this.path = path;
    if (recordId !== undefined) this.recordId = recordId;
    if (cause !== undefined) this.cause = cause;
  }
}

function tmpPrefixFor(recordId) {
  return `.${sanitizeTempPrefix(recordId)}.${randomBytes(8).toString('hex')}${SEGMENT_SUFFIX}`;
}

function sanitizeTempPrefix(recordId) {
  // The temp file name embeds the record id so recovery can attribute an
  // interrupted write to the record that started it.
  return String(recordId).replace(/[^A-Za-z0-9._-]/g, '_');
}

function recordTargetPath(storeRoot, recordId) {
  return join(storeRoot, LOG_DIRNAME, `${sanitizeTempPrefix(recordId)}.json`);
}

function segmentPath(storeRoot) {
  return join(storeRoot, SEGMENT_NAME);
}

function quarantinePath(storeRoot, source, index) {
  const safe = String(source).split(/[\\/]/).pop().replace(/[^A-Za-z0-9._-]/g, '_');
  return join(storeRoot, QUARANTINE_DIRNAME, `${safe}.${index}.quarantined`);
}

function fsyncHandleOrPath(fileSystem, handle, filePath) {
  // Fsync through the handle when the implementation supports it; fall back
  // to opening the path so injected filesystems without handle.sync still
  // get durable ordering where the OS allows it.
  if (handle && typeof handle.sync === 'function') return handle.sync();
  if (typeof fileSystem.open === 'function') {
    return fileSystem.open(filePath, 'r').then((h) => h.sync().finally(() => h.close().catch(() => {})));
  }
  return Promise.resolve();
}

async function fsyncFile(fileSystem, filePath) {
  if (typeof fileSystem.fsync === 'function') return fileSystem.fsync(filePath);
  return fsyncHandleOrPath(fileSystem, null, filePath);
}

async function fsyncDirectory(fileSystem, dirPath) {
  if (typeof fileSystem.fsync === 'function') {
    // Some filesystems expose directory sync through fs.fsync(dir).
    try {
      return await fileSystem.fsync(dirPath);
    } catch (error) {
      if (error.code !== 'EINVAL' && error.code !== 'EBADF' && error.code !== 'ENOTSUP') throw error;
    }
  }
  if (typeof fileSystem.opendir === 'function') {
    const dirHandle = await fileSystem.opendir(dirPath);
    try {
      if (typeof dirHandle.sync === 'function') await dirHandle.sync();
    } finally {
      await dirHandle.close().catch(() => {});
    }
  }
}

/**
 * Validate a provenance record: it must be a plain JSON object with a
 * non-empty string `id` and the current `version`. Anything else is
 * malformed or incompatible and belongs in quarantine, never the log.
 */
export function validateProvenanceRecord(value) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return { ok: false, reason: 'record must be a JSON object' };
  }
  const { id, version, ...body } = value;
  if (typeof id !== 'string' || id.trim() === '') {
    return { ok: false, reason: 'record.id must be a non-empty string' };
  }
  if (id.length > 256 || /[\r\n\0]/.test(id)) {
    return { ok: false, reason: 'record.id is not a safe identifier' };
  }
  if (version !== RECORD_VERSION) {
    return { ok: false, reason: `incompatible record version ${JSON.stringify(version)} (expected ${RECORD_VERSION})` };
  }
  return { ok: true, record: value, body };
}

/**
 * Open a provenance store rooted at `storeRoot`.
 *
 * Options:
 *  - `waitMs` / `staleMs` / `pollMs`: lock acquisition bounds (see repo-locks).
 *  - `holder`, `clock`, `isAlive`, `fileSystem`: repo-locks test seams,
 *    forwarded verbatim so tests can pin time and liveness.
 *
 * The store lazily creates its layout (log/ and quarantine/ directories)
 * on first use; opening an existing store never mutates committed data.
 */
export function openProvenanceStore(storeRoot, options = {}) {
  const {
    fileSystem = fs,
    holder,
    waitMs = 0,
    staleMs = 30_000,
    pollMs = 25,
    clock,
    isAlive,
  } = options;

  const root = storeRoot;

  async function ensureLayout() {
    await fileSystem.mkdir(join(root, LOG_DIRNAME), { recursive: true, mode: 0o700 });
    await fileSystem.mkdir(join(root, QUARANTINE_DIRNAME), { recursive: true, mode: 0o700 });
  }

  async function acquireLock() {
    return acquireRepoLock(LOCK_NAME, {
      lockRoot: root,
      holder,
      waitMs,
      staleMs,
      pollMs,
      clock,
      isAlive,
      fileSystem,
    });
  }

  async function withLock(operation) {
    const lock = await acquireLock();
    try {
      return await operation(lock);
    } finally {
      await lock.release();
    }
  }

  return {
    root,
    lockName: LOCK_NAME,

    /**
     * Acquire the store lock, run `operation`, release it. The lock is held
     * across the whole operation — this is what makes append-safe writes
     * safe: no second writer can interleave a rename or an append mid-write.
     */
    withLock,

    /**
     * Crash recovery, run under the store lock.
     *
     * Scans `log/` for interrupted atomic writes (`.<id>.<nonce>.tmp`):
     * every one is reported as `{ recordId, incomplete: true, path }` and
     * removed. A temp file means the write never reached the atomic rename,
     * so success is never claimed for it. If the target record already
     * exists, the interrupted attempt is simply garbage; if it does not,
     * the caller retries the same record id and the write is idempotent.
     *
     * Also reconciles the append-only segment: leftover segment temp files
     * (`provenance.log.*.tmp`) from an interrupted append are reported as
     * incomplete and removed; the segment itself is only ever replaced by
     * an atomic rename, so it is always complete.
     *
     * Returns `{ recovered: [...], interrupted: [...] }`.
     */
    async recover() {
      return withLock(async () => {
        await ensureLayout();
        const recovered = [];
        const interrupted = [];

        const logEntries = await fileSystem.readdir(join(root, LOG_DIRNAME));
        for (const entry of logEntries) {
          if (!entry.endsWith(SEGMENT_SUFFIX) || !entry.startsWith('.')) continue;
          const tmpPath = join(root, LOG_DIRNAME, entry);
          // `.name.<nonce>.tmp` — the record id sits between the leading
          // dot and the nonce suffix.
          const m = entry.match(/^(.*)\.[0-9a-f]{16,}\.tmp$/);
          // The temp file name embeds the record id between the leading dot
          // (hidden-file marker) and the nonce suffix; strip the marker so
          // recovery attributes the interrupted write to the record id.
          const recordId = (m ? m[1] : entry).replace(/^\./, '');
          interrupted.push({ recordId, incomplete: true, path: tmpPath, kind: 'record-write' });
          await fileSystem.rm(tmpPath, { force: true });
          recovered.push({ recordId, action: 'removed-interrupted-write' });
        }

        const rootEntries = await fileSystem.readdir(root);
        for (const entry of rootEntries) {
          if (!entry.startsWith(`${SEGMENT_NAME}.`)) continue;
          const tmpPath = join(root, entry);
          interrupted.push({ recordId: null, incomplete: true, path: tmpPath, kind: 'segment-append' });
          await fileSystem.rm(tmpPath, { force: true });
          recovered.push({ recordId: null, action: 'removed-interrupted-append' });
        }

        return { recovered, interrupted };
      });
    },

    /**
     * Atomically persist one provenance record (under the store lock).
     *
     * Sequence: validate -> temp create (wx) in log/ -> write JSON ->
     * fsync file -> close -> rename over `<root>/log/<id>.json` ->
     * fsync log/ directory -> append one line to the segment ->
     * fsync segment -> fsync store root. The lock is released only after
     * every fsync, so an observer that later acquires the lock sees a
     * complete record or nothing.
     *
     * A record id already on disk is rejected with `E_DUPLICATE_RECORD`
     * (idempotent retry semantics: the first complete write wins; an
     * interrupted write was removed by `recover()` and is accepted on
     * retry).
     */
    async appendRecord(record) {
      const verdict = validateProvenanceRecord(record);
      if (!verdict.ok) {
        throw new ProvenanceStoreError(
          `refusing to append an invalid record: ${verdict.reason}`,
          { code: 'E_INVALID_RECORD', recordId: record && typeof record === 'object' ? record.id : undefined });
      }
      const recordId = verdict.record.id;
      const target = recordTargetPath(root, recordId);

      return withLock(async () => {
        await ensureLayout();
        let existing = null;
        try {
          existing = JSON.parse(await fileSystem.readFile(target, 'utf8'));
        } catch (error) {
          if (error.code !== 'ENOENT') {
            throw new ProvenanceStoreError(
              `record ${target} exists but is unreadable (${error.code ?? error.message}); ` +
              'recover or quarantine it before retrying this id.',
              { code: 'E_RECORD_UNREADABLE', path: target, recordId, cause: error });
          }
        }
        if (existing !== null) {
          const existingVerdict = validateProvenanceRecord(existing);
          if (existingVerdict.ok && existingVerdict.record.id === recordId) {
            throw new ProvenanceStoreError(
              `record id "${recordId}" already has a committed provenance record; ` +
              'retrying the same id is a no-op that the caller must treat as already-accepted.',
              { code: 'E_DUPLICATE_RECORD', path: target, recordId });
          }
          throw new ProvenanceStoreError(
            `record id "${recordId}" collides with a different or corrupt committed record; ` +
            'quarantine the existing entry before writing a new one.',
            { code: 'E_RECORD_CONFLICT', path: target, recordId });
        }

        const tmp = join(root, LOG_DIRNAME, tmpPrefixFor(recordId));
        let handle;
        try {
          handle = await fileSystem.open(tmp, 'wx', 0o600);
          await handle.writeFile(`${JSON.stringify(verdict.record)}\n`, 'utf8');
          await fsyncHandleOrPath(fileSystem, handle, tmp);
          await handle.close();
          handle = undefined;
        } catch (error) {
          await handle?.close().catch(() => {});
          await fileSystem.rm(tmp, { force: true }).catch(() => {});
          throw new ProvenanceStoreError(
            `atomic write of record ${recordId} failed before the rename; no partial record was published.`,
            { code: 'E_RECORD_WRITE', path: tmp, recordId, cause: error });
        }
        try {
          await fileSystem.rename(tmp, target);
        } catch (error) {
          // A second writer raced us between the existence check and the
          // rename; the exclusive temp create above means the loser simply
          // discards its copy. Report it as an idempotent no-op.
          const raced = error.code === 'EEXIST' || error.code === 'ENOENT';
          await fileSystem.rm(tmp, { force: true }).catch(() => {});
          if (raced) {
            throw new ProvenanceStoreError(
              `record id "${recordId}" was committed by a concurrent writer first; ` +
              'this retry is an idempotent no-op.',
              { code: 'E_DUPLICATE_RECORD', path: target, recordId, cause: error });
          }
          throw new ProvenanceStoreError(
            `atomic rename of record ${recordId} failed (${error.code ?? error.message}); ` +
            'the log is untouched and the temp file was removed.',
            { code: 'E_RECORD_RENAME', path: target, recordId, cause: error });
        }
        await fsyncDirectory(fileSystem, join(root, LOG_DIRNAME));

        await appendSegmentLine(root, JSON.stringify(verdict.record), { fileSystem });
        return { recordId, path: target };
      });
    },

    /**
     * Read all committed, valid provenance records, in stable id order.
     * Malformed or incompatible entries in the record log are skipped here
     * (and quarantined by `repair()`); the append-only segment is treated
     * as an index only — the record files under log/ are the source of
     * truth, so a torn or missing segment line never hides a valid record.
     */
    async readAll() {
      const records = [];
      const skipped = [];
      const logDir = join(root, LOG_DIRNAME);
      let entries;
      try {
        entries = await fileSystem.readdir(logDir);
      } catch (error) {
        if (error.code === 'ENOENT') return { records, skipped };
        throw new ProvenanceStoreError(
          `log directory ${logDir} could not be read (${error.code ?? error.message}).`,
          { code: 'E_LOG_DIR', path: logDir, cause: error });
      }
      const files = entries.filter((entry) => entry.endsWith('.json') && !entry.startsWith('.'));
      for (const entry of files.sort()) {
        const file = join(logDir, entry);
        const recordId = entry.slice(0, -'.json'.length);
        let parsed;
        try {
          parsed = JSON.parse(await fileSystem.readFile(file, 'utf8'));
        } catch (error) {
          skipped.push({ recordId, reason: `unreadable or corrupt JSON (${error.code ?? error.message})`, path: file });
          continue;
        }
        const verdict = validateProvenanceRecord(parsed);
        if (!verdict.ok) {
          skipped.push({ recordId, reason: verdict.reason, path: file });
          continue;
        }
        if (verdict.record.id !== recordId) {
          skipped.push({ recordId, reason: 'record id does not match its file name', path: file });
          continue;
        }
        records.push(verdict.record);
      }
      return { records, skipped };
    },

    /**
     * Quarantine malformed or incompatible entries (under the store lock).
     *
     * Every skipped entry from `readAll()` is moved, unchanged, into
     * `quarantine/`; the log is never truncated or rewritten in place —
     * each bad entry is unlinked only after its copy lands in quarantine
     * (copy then verify-then-unlink). Valid records around them are
     * untouched and stay readable.
     */
    async repair() {
      return withLock(async () => {
        await ensureLayout();
        const { records, skipped } = await scanLog(root, fileSystem);
        const quarantined = [];
        for (const [index, entry] of skipped.entries()) {
          const dest = quarantinePath(root, entry.path, index + 1);
          const tmp = join(root, QUARANTINE_DIRNAME, `${randomBytes(8).toString('hex')}.tmp`);
          const content = await fileSystem.readFile(entry.path);
          let handle;
          try {
            handle = await fileSystem.open(tmp, 'wx', 0o600);
            await handle.writeFile(content);
            await fsyncHandleOrPath(fileSystem, handle, tmp);
            await handle.close();
            handle = undefined;
            await fileSystem.rename(tmp, dest);
          } catch (error) {
            await handle?.close().catch(() => {});
            await fileSystem.rm(tmp, { force: true }).catch(() => {});
            throw new ProvenanceStoreError(
              `quarantine of ${entry.path} failed (${error.code ?? error.message}); the log was not modified.`,
              { code: 'E_QUARANTINE', path: dest, cause: error });
          }
          await fileSystem.rm(entry.path, { force: true });
          await fsyncDirectory(fileSystem, join(root, QUARANTINE_DIRNAME));
          quarantined.push({ recordId: entry.recordId, reason: entry.reason, from: entry.path, to: dest });
        }
        if (quarantined.length) {
          await fsyncDirectory(fileSystem, join(root, LOG_DIRNAME));
        }
        return { records: records.length, quarantined };
      });
    },
  };
}

/**
 * Scan the record log, splitting committed entries into valid records and
 * skip entries (malformed, incompatible, or id-mismatched). Called by both
 * readAll() (lock-free read path) and repair() (already under the lock).
 */
async function scanLog(root, fileSystem) {
  const records = [];
  const skipped = [];
  const logDir = join(root, LOG_DIRNAME);
  let entries;
  try {
    entries = await fileSystem.readdir(logDir);
  } catch (error) {
    if (error.code === 'ENOENT') return { records, skipped };
    throw new ProvenanceStoreError(
      `log directory ${logDir} could not be read (${error.code ?? error.message}).`,
      { code: 'E_LOG_DIR', path: logDir, cause: error });
  }
  const files = entries.filter((entry) => entry.endsWith('.json') && !entry.startsWith('.'));
  for (const entry of files.sort()) {
    const file = join(logDir, entry);
    const recordId = entry.slice(0, -'.json'.length);
    let parsed;
    try {
      parsed = JSON.parse(await fileSystem.readFile(file, 'utf8'));
    } catch (error) {
      skipped.push({ recordId, reason: `unreadable or corrupt JSON (${error.code ?? error.message})`, path: file });
      continue;
    }
    const verdict = validateProvenanceRecord(parsed);
    if (!verdict.ok) {
      skipped.push({ recordId, reason: verdict.reason, path: file });
      continue;
    }
    if (verdict.record.id !== recordId) {
      skipped.push({ recordId, reason: 'record id does not match its file name', path: file });
      continue;
    }
    records.push(verdict.record);
  }
  return { records, skipped };
}

async function readAllInternal({ root, fileSystem }) {
  // Internal duplicate of readAll without a lock: called from repair(),
  // which already holds the store lock (re-entrant withLock would deadlock
  // on the exclusive create).
  const records = [];
  const skipped = [];
  const logDir = join(root, LOG_DIRNAME);
  let entries;
  try {
    entries = await fileSystem.readdir(logDir);
  } catch (error) {
    if (error.code === 'ENOENT') return { records, skipped };
    throw error;
  }
  const files = entries.filter((entry) => entry.endsWith('.json') && !entry.startsWith('.'));
  for (const entry of files.sort()) {
    const file = join(logDir, entry);
    const recordId = entry.slice(0, -'.json'.length);
    let parsed;
    try {
      parsed = JSON.parse(await fileSystem.readFile(file, 'utf8'));
    } catch (error) {
      skipped.push({ recordId, reason: `unreadable or corrupt JSON (${error.code ?? error.message})`, path: file });
      continue;
    }
    const verdict = validateProvenanceRecord(parsed);
    if (!verdict.ok) {
      skipped.push({ recordId, reason: verdict.reason, path: file });
      continue;
    }
    if (verdict.record.id !== recordId) {
      skipped.push({ recordId, reason: 'record id does not match its file name', path: file });
      continue;
    }
    records.push(verdict.record);
  }
  return { records, skipped };
}

/**
 * Append one line to the append-only segment and fsync it (and the store
 * root) before the caller releases the lock. A torn append never affects
 * correctness: the segment is an index, and `repair()` rebuilds it from the
 * record files when lines do not parse.
 */
async function appendSegmentLine(storeRoot, line, { fileSystem }) {
  const target = segmentPath(storeRoot);
  const tmp = join(storeRoot, `${SEGMENT_NAME}.${randomBytes(8).toString('hex')}${SEGMENT_SUFFIX}`);
  let existing = '';
  try {
    existing = await fileSystem.readFile(target, 'utf8');
  } catch (error) {
    if (error.code !== 'ENOENT') {
      throw new ProvenanceStoreError(
        `segment ${target} is unreadable (${error.code ?? error.message}); the log record files remain authoritative.`,
        { code: 'E_SEGMENT_UNREADABLE', path: target, cause: error });
    }
  }
  const content = `${existing}${line}\n`;
  let handle;
  try {
    handle = await fileSystem.open(tmp, 'wx', 0o600);
    await handle.writeFile(content, 'utf8');
    await fsyncHandleOrPath(fileSystem, handle, tmp);
    await handle.close();
    handle = undefined;
    await fileSystem.rename(tmp, target);
  } catch (error) {
    await handle?.close().catch(() => {});
    await fileSystem.rm(tmp, { force: true }).catch(() => {});
    throw new ProvenanceStoreError(
      `segment append failed (${error.code ?? error.message}); the segment index is advisory only.`,
      { code: 'E_SEGMENT_APPEND', path: target, cause: error });
  }
  await fsyncDirectory(fileSystem, storeRoot);
}
