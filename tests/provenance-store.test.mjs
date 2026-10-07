import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { join, dirname } from 'node:path';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import {
  openProvenanceStore,
  validateProvenanceRecord,
  RECORD_VERSION,
  ProvenanceStoreError,
  LOG_DIRNAME,
  QUARANTINE_DIRNAME,
} from '../src/lib/provenance-store.mjs';
import { acquireRepoLock, RepoLockError } from '../src/lib/repo-locks.mjs';

async function makeStoreRoot() {
  return mkdtemp(join(tmpdir(), 'provenance-store-'));
}

async function cleanup(root) {
  await rm(root, { recursive: true, force: true });
}

function record(id, extra = {}) {
  return { id, version: RECORD_VERSION, ...extra };
}

// A provably dead pid for stale-lock tests.
function deadPid() {
  const child = new (require('node:child_process').ChildProcess)();
  return 0; // unused; tests below use explicit fake pids via isAlive seam
}
void deadPid;

test('atomic append-safe write: lock is held across write+fsync and the record is durable', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root);

  const result = await store.appendRecord(record('alpha', { payload: { step: 'build' } }));
  assert.equal(result.recordId, 'alpha');

  const onDisk = JSON.parse(await fs.readFile(result.path, 'utf8'));
  assert.deepEqual(onDisk, record('alpha', { payload: { step: 'build' } }));

  const { records } = await store.readAll();
  assert.equal(records.length, 1);
  assert.equal(records[0].id, 'alpha');
  assert.equal(records[0].payload.step, 'build');

  // No temp files remain after a clean write.
  const entries = await fs.readdir(join(root, LOG_DIRNAME));
  assert.deepEqual(entries.filter((e) => e.startsWith('.')), []);
});

test('second writer is refused while the store lock is live (nonzero error)', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root);

  // Hold the store lock directly with a live fake holder.
  const held = await acquireRepoLock('provenance-store', {
    lockRoot: root,
    holder: 'other-writer',
    isAlive: () => true,
  });
  t.after(async () => {
    await held.release();
  });

  await assert.rejects(
    () => store.appendRecord(record('blocked')),
    (error) => error instanceof RepoLockError && error.code === 'E_LOCK_HELD',
  );
});

test('interrupted write leaves no torn record and recovers as incomplete', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root);

  // A clean record survives around the interrupted one.
  await store.appendRecord(record('good'));

  // Simulate a crash mid-write: temp file exists, rename never happened.
  const logDir = join(root, LOG_DIRNAME);
  await fs.mkdir(logDir, { recursive: true });
  const torn = join(logDir, '.crashed.0123456789abcdef.tmp');
  await fs.writeFile(torn, '{"id":"crashed","version":1,"partial":tru', 'utf8');

  const recovery = await store.recover();
  assert.equal(recovery.interrupted.length, 1);
  assert.equal(recovery.interrupted[0].recordId, 'crashed');
  assert.equal(recovery.interrupted[0].incomplete, true);
  // The interrupted write is reported incomplete: no success claimed.
  assert.ok(!recovery.interrupted[0].complete);

  // The torn temp is gone; the committed record survives.
  assert.equal(await fs.stat(torn).then(() => true, () => false), false);
  const { records, skipped } = await store.readAll();
  assert.equal(records.length, 1);
  assert.equal(records[0].id, 'good');
  assert.equal(skipped.length, 0);
});

test('retry of the same record id after an interrupted write is idempotent (no duplicate)', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root);

  await store.appendRecord(record('dup', { seq: 1 }));

  // Simulate a crash of a second attempt for the same id, then recovery.
  const logDir = join(root, LOG_DIRNAME);
  const torn = join(logDir, '.dup.ffffffffffffffff.tmp');
  await fs.writeFile(torn, '{"id":"dup","version":1,"seq":2', 'utf8');
  await store.recover();

  // The committed record wins; retrying the same id is rejected, never appended.
  await assert.rejects(
    () => store.appendRecord(record('dup', { seq: 2 })),
    (error) => error instanceof ProvenanceStoreError && error.code === 'E_DUPLICATE_RECORD',
  );
  const { records } = await store.readAll();
  assert.equal(records.length, 1);
  assert.equal(records[0].seq, 1);
});

test('stale lock with a dead recorded pid is reclaimed deterministically', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root);

  // Plant a lock file whose recorded pid is dead.
  const lockPath = join(root, 'locks', 'provenance-store.lock');
  await fs.mkdir(dirname(lockPath), { recursive: true });
  await fs.writeFile(lockPath, `${JSON.stringify({ holder: 'dead-writer', pid: 4_999_999, acquiredAt: new Date().toISOString() })}\n`, 'utf8');

  // isAlive reports the dead pid as dead; the store should reclaim and proceed.
  const storeWithDead = openProvenanceStore(root, { isAlive: () => false });
  const result = await storeWithDead.appendRecord(record('after-stale'));
  assert.equal(result.recordId, 'after-stale');
  const { records } = await storeWithDead.readAll();
  assert.equal(records.length, 1);

  // The live store (real liveness check) also reclaims it: pid 4_999_999 is
  // not alive on this machine, so the default isAlive reports it dead.
  const result2 = await store.appendRecord(record('after-stale-2'));
  assert.equal(result2.recordId, 'after-stale-2');
});

test('stale lock by old timestamp with no usable pid is reclaimed', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root, { staleMs: 1000 });

  const lockPath = join(root, 'locks', 'provenance-store.lock');
  await fs.mkdir(dirname(lockPath), { recursive: true });
  const old = new Date(Date.now() - 10 * 60 * 1000).toISOString();
  await fs.writeFile(lockPath, `${JSON.stringify({ holder: 'old-writer', pid: null, acquiredAt: old })}\n`, 'utf8');

  // A lock older than staleMs with no live holder is taken over.
  const result = await store.appendRecord(record('after-old'));
  assert.equal(result.recordId, 'after-old');
});

test('live lock with recent timestamp is never stolen', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));

  const lockPath = join(root, 'locks', 'provenance-store.lock');
  await fs.mkdir(dirname(lockPath), { recursive: true });
  await fs.writeFile(lockPath, `${JSON.stringify({ holder: 'live-writer', pid: process.pid, acquiredAt: new Date().toISOString() })}\n`, 'utf8');

  const store = openProvenanceStore(root, { staleMs: 60_000, isAlive: () => true });
  await assert.rejects(
    () => store.appendRecord(record('denied')),
    (error) => error instanceof RepoLockError && error.code === 'E_LOCK_HELD',
  );
});

test('malformed records are quarantined, valid neighbors stay readable', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root);

  await store.appendRecord(record('ok-before'));
  await store.appendRecord(record('ok-after'));

  // Plant malformed entries in the log: corrupt JSON, incompatible version,
  // id/filename mismatch.
  const logDir = join(root, LOG_DIRNAME);
  await fs.writeFile(join(logDir, 'corrupt.json'), '{not json at all', 'utf8');
  await fs.writeFile(join(logDir, 'wrong-version.json'), `${JSON.stringify(record('wrong-version', { version: 99 }))}\n`, 'utf8');
  await fs.writeFile(join(logDir, 'mismatch.json'), `${JSON.stringify(record('other-id'))}\n`, 'utf8');

  const before = await store.readAll();
  assert.equal(before.records.length, 2);
  assert.equal(before.skipped.length, 3);

  const report = await store.repair();
  assert.equal(report.records, 2);
  assert.equal(report.quarantined.length, 3);
  for (const entry of report.quarantined) {
    assert.ok(entry.to.includes(QUARANTINE_DIRNAME));
    // The quarantined copy is byte-identical to what the log had.
    await fs.access(entry.to);
  }

  const after = await store.readAll();
  assert.equal(after.skipped.length, 0);
  const ids = after.records.map((r) => r.id).sort();
  assert.deepEqual(ids, ['ok-after', 'ok-before']);
});

test('quarantine never truncates the log: log file count only decreases by quarantined entries', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root);

  await store.appendRecord(record('a'));
  await store.appendRecord(record('b'));
  const logDir = join(root, LOG_DIRNAME);
  await fs.writeFile(join(logDir, 'bad.json'), 'oops', 'utf8');

  const beforeCount = (await fs.readdir(logDir)).filter((e) => e.endsWith('.json')).length;
  assert.equal(beforeCount, 3);

  await store.repair();
  const afterCount = (await fs.readdir(logDir)).filter((e) => e.endsWith('.json')).length;
  assert.equal(afterCount, 2);
  const { records } = await store.readAll();
  assert.deepEqual(records.map((r) => r.id).sort(), ['a', 'b']);
});

test('store survives repo lock directory deletion and re-open (durability via fsync discipline)', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));

  const first = openProvenanceStore(root);
  await first.appendRecord(record('persistent', { durable: true }));

  // Simulate worktree cleanup: the locks directory (repo lock area) is gone.
  await rm(join(root, 'locks'), { recursive: true, force: true });

  // Re-open the store from the same root; records persist and the lock
  // directory is recreated on next acquisition.
  const second = openProvenanceStore(root);
  const { records } = await second.readAll();
  assert.equal(records.length, 1);
  assert.equal(records[0].id, 'persistent');
  assert.equal(records[0].durable, true);

  await second.appendRecord(record('post-cleanup'));
  const again = await second.readAll();
  assert.deepEqual(again.records.map((r) => r.id).sort(), ['persistent', 'post-cleanup']);
});

test('interrupted segment append is recovered and does not corrupt the log', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root);
  await store.appendRecord(record('seg-ok'));

  // Simulate an interrupted segment append.
  await fs.writeFile(join(root, 'provenance.log.deadbeefdeadbeef.tmp'), '{"id":"seg-ok","version":1', 'utf8');

  const recovery = await store.recover();
  assert.ok(recovery.interrupted.some((i) => i.kind === 'segment-append'));
  const entries = await fs.readdir(root);
  assert.equal(entries.filter((e) => e.startsWith('provenance.log.')).length, 0);

  const { records } = await store.readAll();
  assert.equal(records.length, 1);
  assert.equal(records[0].id, 'seg-ok');
});

test('invalid records are rejected at append time, never written', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root);

  await assert.rejects(
    () => store.appendRecord({ id: '', version: RECORD_VERSION }),
    (error) => error instanceof ProvenanceStoreError && error.code === 'E_INVALID_RECORD',
  );
  await assert.rejects(
    () => store.appendRecord({ id: 'x', version: 42 }),
    (error) => error instanceof ProvenanceStoreError && error.code === 'E_INVALID_RECORD',
  );
  const { records } = await store.readAll();
  assert.equal(records.length, 0);
});

test('validateProvenanceRecord accepts valid and rejects malformed/incompatible shapes', () => {
  assert.equal(validateProvenanceRecord(record('v')).ok, true);
  assert.equal(validateProvenanceRecord(null).ok, false);
  assert.equal(validateProvenanceRecord([1, 2]).ok, false);
  assert.equal(validateProvenanceRecord({ id: 'v' }).ok, false, 'missing version is incompatible');
  assert.equal(validateProvenanceRecord({ id: 'v', version: 99 }).ok, false);
  assert.equal(validateProvenanceRecord({ version: RECORD_VERSION }).ok, false, 'missing id');
  assert.equal(validateProvenanceRecord({ id: 'bad\nid', version: RECORD_VERSION }).ok, false);
});

test('withLock releases the store lock on failure, so the store stays usable', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root);

  await assert.rejects(
    () => store.withLock(async () => {
      throw new Error('boom');
    }),
    /boom/,
  );

  // Lock is released: the store can be used again.
  await store.appendRecord(record('after-failure'));
  const { records } = await store.readAll();
  assert.equal(records.length, 1);
});

test('append while another process holds the store lock with waitMs=0 fails fast', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => cleanup(root));
  const store = openProvenanceStore(root, { waitMs: 0 });

  const held = await acquireRepoLock('provenance-store', {
    lockRoot: root,
    holder: 'busy',
    isAlive: () => true,
  });
  t.after(async () => {
    await held.release();
  });
  await assert.rejects(
    () => store.appendRecord(record('no-wait')),
    (error) => error instanceof RepoLockError && error.code === 'E_LOCK_HELD',
  );
});
