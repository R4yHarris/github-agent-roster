// Provenance recovery edge cases (parent #195): crash recovery (interrupted
// atomic writes, idempotent retry), stale-lock recovery with a deterministic
// injected clock, and recovery of the full lifecycle edge cases — requested
// vs served model discrepancy, unknown usage, failure, cancellation — all
// with injected clock/IDs so every assertion is deterministic.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  openProvenanceStore,
  ProvenanceStoreError,
  LOG_DIRNAME,
  SEGMENT_NAME,
  SEGMENT_SUFFIX,
} from '../src/lib/provenance-store.mjs';
import { createProvenanceStore } from '../src/lib/provenance-api.mjs';

// Fixed, deterministic clock: every lock timestamp and record createdAt in
// this file is derived from these values.
const T0 = 1735689600000; // 2025-01-01T00:00:00Z
let now = T0;
const clock = () => now;
const tick = (ms) => { now += ms; };

const RUN_ID = 'run-2025-0101-700';
const SESSION_ID = 'sess-2025-0101-700';

async function makeStoreRoot() {
  return fs.mkdtemp(join(tmpdir(), 'prov-recovery-'));
}

function makeRecord(id, payload = {}) {
  return { id, version: 1, runId: RUN_ID, sessionId: SESSION_ID, event: 'started', payload, createdAt: now };
}

function textTree(value) {
  if (typeof value === 'string') return value;
  if (Array.isArray(value)) return value.map(textTree).join('\n');
  if (value && typeof value === 'object') {
    return Object.entries(value).map(([k, v]) => `${k}\n${textTree(v)}`).join('\n');
  }
  return String(value);
}

test('crash recovery: interrupted atomic write is reported incomplete and removed; retry of the same id succeeds exactly once', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const store = openProvenanceStore(root, { clock });
  const recordId = 'recovered-record-0001';

  // Simulate a crash mid-write: a temp file exists in log/ for a record that
  // never reached the atomic rename. The name embeds the record id between
  // the leading dot and the 16-hex nonce suffix.
  const tmpName = `.${recordId}.0123456789abcdef${SEGMENT_SUFFIX}`;
  await fs.mkdir(join(root, LOG_DIRNAME), { recursive: true });
  await fs.writeFile(join(root, LOG_DIRNAME, tmpName), '{"partial":', 'utf8');

  const { recovered, interrupted } = await store.recover();
  assert.equal(interrupted.length, 1, 'the interrupted write is reported');
  assert.equal(interrupted[0].recordId, recordId, 'attributed to the interrupted record id');
  assert.equal(interrupted[0].incomplete, true, 'reported as incomplete, never as success');
  assert.equal(interrupted[0].kind, 'record-write');
  assert.equal(recovered.length, 1);
  assert.equal(recovered[0].recordId, recordId);
  assert.equal(recovered[0].action, 'removed-interrupted-write');
  await assert.rejects(fs.stat(join(root, LOG_DIRNAME, tmpName)), { code: 'ENOENT' }, 'temp file removed');
  const noCommitted = join(root, LOG_DIRNAME, `${recordId}.json`);
  await assert.rejects(fs.stat(noCommitted), { code: 'ENOENT' }, 'no committed record is ever claimed');

  // Retry of the same id after recovery is accepted (idempotent retry).
  await store.appendRecord(makeRecord(recordId));
  const firstRead = await store.readAll();
  assert.equal(firstRead.records.length, 1);
  assert.equal(firstRead.records[0].id, recordId);

  // A duplicate append of the same id is rejected, not written twice.
  await assert.rejects(
    store.appendRecord(makeRecord(recordId)),
    (error) => error instanceof ProvenanceStoreError && error.code === 'E_DUPLICATE_RECORD',
    'second append of the same id is an idempotent no-op',
  );
  const secondRead = await store.readAll();
  assert.equal(secondRead.records.length, 1, 'never accepted twice');
  assert.deepEqual(secondRead.records[0].payload, makeRecord(recordId).payload);
});

test('crash recovery: leftover segment temp file from an interrupted append is removed; committed records survive', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const store = openProvenanceStore(root, { clock });

  await store.appendRecord(makeRecord('seg-record-0001', { note: 'keep me' }));
  // Simulate a crash during the segment rewrite (tmp file, no rename).
  const segTmp = join(root, `${SEGMENT_NAME}.fedcba9876543210${SEGMENT_SUFFIX}`);
  await fs.writeFile(segTmp, '{"stale":true}\n', 'utf8');

  const { recovered, interrupted } = await store.recover();
  const segmentInterrupted = interrupted.filter((entry) => entry.kind === 'segment-append');
  assert.equal(segmentInterrupted.length, 1, 'interrupted segment append reported');
  assert.equal(segmentInterrupted[0].incomplete, true);
  assert.ok(recovered.some((entry) => entry.action === 'removed-interrupted-append'));
  await assert.rejects(fs.stat(segTmp), { code: 'ENOENT' }, 'stale segment temp removed');

  const { records } = await store.readAll();
  assert.equal(records.length, 1, 'committed record unaffected by torn segment');
  assert.equal(records[0].id, 'seg-record-0001');
  assert.equal(records[0].payload.note, 'keep me');
});

// Plant the lock a crashed writer left behind; the store must refuse or take it over.
async function plantLock(root, record) {
  await fs.mkdir(join(root, 'locks'), { recursive: true });
  await fs.writeFile(join(root, 'locks', 'provenance-store.lock'), JSON.stringify(record), 'utf8');
}

test('stale lock: a held lock is refused until the injected clock passes staleMs, then taken over', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const staleMs = 1000;
  // A pid-less lock from a crashed writer: only the injected clock can make it stale.
  await plantLock(root, { holder: 'crashed-writer', acquiredAt: new Date(now).toISOString() });
  const store = openProvenanceStore(root, { holder: 'recovery-writer', staleMs, clock, isAlive: () => true });

  tick(staleMs - 1);
  await assert.rejects(store.appendRecord(makeRecord('blocked-0001')),
    (error) => error.code === 'E_LOCK_HELD' && error.holder === 'crashed-writer', 'fresh lock refuses a second writer');
  assert.equal((await store.readAll()).records.length, 0, 'the refused append wrote nothing');

  tick(2);
  await store.appendRecord(makeRecord('post-stale-0001', { writtenAt: now }));
  const { records } = await store.readAll();
  assert.deepEqual(records.map((record) => record.id), ['post-stale-0001'], 'takeover lets the append commit');
  assert.equal(records[0].payload.writtenAt, T0 + staleMs + 1, 'record stamped by injected clock');
  await assert.rejects(fs.stat(join(root, 'locks', 'provenance-store.lock')), { code: 'ENOENT' },
    'the taken-over lock is released after the append');
});

test('stale lock: a live holder is never taken over; a provably dead holder is', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const holderPid = 424242;
  const alive = new Set([holderPid]);
  await plantLock(root, { holder: 'pid-424242', pid: holderPid, acquiredAt: new Date(now).toISOString() });
  const store = openProvenanceStore(root, {
    holder: 'recovery-writer', staleMs: 1000, clock, isAlive: (pid) => alive.has(pid),
  });

  tick(60_000);
  await assert.rejects(store.appendRecord(makeRecord('blocked-0002')),
    (error) => error.code === 'E_LOCK_HELD' && error.holder === 'pid-424242', 'old but live lock is not stolen');

  alive.delete(holderPid);
  await store.appendRecord(makeRecord('after-dead-0002'));
  assert.deepEqual((await store.readAll()).records.map((record) => record.id), ['after-dead-0002']);
});
test('recovered lifecycle: requested-vs-served discrepancy, unknown usage, failure, and cancellation are durable and readable', async (t) => {
  const root = await makeStoreRoot();
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const store = createProvenanceStore({ root, repoRoot: process.cwd() });

  // A crash between the 'started' write and the rest: leave an interrupted
  // write, recover, then complete the lifecycle deterministically.
  const startedId = (await store.recordEvent({
    runId: RUN_ID,
    sessionId: SESSION_ID,
    event: 'started',
    payload: { requestedModel: 'gpt-large-preview', servedModel: 'gpt-large-stable' },
  })).id;
  assert.ok(startedId, 'started record committed');

  const interruptedId = 'sess-700-failure';
  const tmpName = `.${interruptedId}.abcdefabcdefabcd${SEGMENT_SUFFIX}`;
  await fs.mkdir(join(root, LOG_DIRNAME), { recursive: true, mode: 0o700 });
  await fs.writeFile(join(root, LOG_DIRNAME, tmpName), '{"in', 'utf8');

  const { recover } = openProvenanceStore(root, { clock });
  const { recovered } = await recover();
  assert.ok(recovered.some((entry) => entry.recordId === interruptedId), 'interrupted failure write recovered');

  tick(2000);
  // Unknown usage: metrics with missing/invalid values plus an unknown field.
  const failure = await store.recordEvent({
    runId: RUN_ID,
    sessionId: SESSION_ID,
    event: 'failure',
    payload: {
      requestedModel: 'gpt-large-preview',
      servedModel: 'gpt-large-stable',
      modelDiscrepancy: true,
      error: 'upstream 500',
      metrics: {
        duration_ms: 950,
        cost_usd: null,
        tokens_prompt: 'N/A',
        speculative_cache: 2,
      },
    },
  });
  assert.equal(failure.payload.requestedModel, 'gpt-large-preview');
  assert.equal(failure.payload.servedModel, 'gpt-large-stable');
  assert.notEqual(failure.payload.requestedModel, failure.payload.servedModel, 'discrepancy preserved');
  assert.equal(failure.payload.metrics.duration_ms, 950);
  assert.equal(failure.payload.metrics.cost_usd, null, 'unknown usage carried through as-is (not a string sentinel)');

  tick(1500);
  await store.recordEvent({
    runId: RUN_ID,
    sessionId: SESSION_ID,
    event: 'cancellation',
    payload: { requestedModel: 'gpt-large-preview', servedModel: 'gpt-large-stable' },
  });

  // Crash between the cancellation append and its directory fsync: a second
  // interrupted write appears; recovery cleans it without losing data.
  const secondInterruptedId = 'sess-700-cancellation';
  await fs.writeFile(
    join(root, LOG_DIRNAME, `.${secondInterruptedId}.1234567890abcdef${SEGMENT_SUFFIX}`),
    '{"in',
    'utf8',
  );
  const second = await recover();
  assert.ok(second.recovered.length >= 1, 'second crash marker recovered');

  // The durable surface: read all committed records through the earlier-wave
  // store API backing the typed facade.
  const { readAll } = openProvenanceStore(root, { clock });

  const events = await store.query({ runId: RUN_ID, sessionId: SESSION_ID });
  const seen = events.map((record) => record.event);
  assert.deepEqual(seen.sort(), ['cancellation', 'failure', 'started'], 'all lifecycle events durable after recovery');
  const persistedFailure = events.find((record) => record.event === 'failure');
  assert.equal(persistedFailure.payload.modelDiscrepancy, true, 'failure record intact after recovery');
  const persistedCancellation = events.find((record) => record.event === 'cancellation');
  assert.ok(persistedCancellation, 'cancellation record survived recovery');

  // The interrupted record id was never committed (no partial success).
  const { records } = await readAll();
  assert.ok(
    !records.some((record) => record.id === interruptedId || record.id === secondInterruptedId),
    'interrupted writes are never accepted as committed',
  );

  void textTree;
});
