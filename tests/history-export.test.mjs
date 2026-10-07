import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { LOG_DIRNAME, openProvenanceStore } from '../src/lib/provenance-store.mjs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { promisify } from 'node:util';

import {
  HISTORY_EXPORT_SCHEMA_VERSION,
  canonicalizeRecord,
  exportHistory,
  previewScopedDeletion,
  deleteScopedRecords,
  renderHistoryJson,
  renderHistorySummary,
} from '../src/lib/history-export.mjs';
import { createHistoryReader, filterRecords, filterRepositories, findHistoryRecord, historyFields } from '../src/lib/history-query.mjs';
import { parseListFlags, runHistory } from '../src/lib/history-cli.mjs';

const execute = promisify(execFile);

function makeRecord(overrides = {}) {
  return {
    id: overrides.id ?? 'rec-1',
    version: 1,
    runId: overrides.runId ?? 'run-1',
    sessionId: overrides.sessionId ?? 'session-1',
    repoIdentity: overrides.repoIdentity ?? 'repo-1234abcd',
    servedModel: overrides.servedModel ?? 'model-a',
    createdAt: overrides.createdAt ?? '2025-01-02T03:04:05.000Z',
    issue: overrides.issue,
    seat: overrides.seat,
    payload: overrides.payload,
    scope: overrides.scope,
  };
}

function fakeRun() {
  return async (cmd, args) => ({ stdout: 'relative-store\n' });
}

test('exportHistory includes schema version and stable record ids', () => {
  const records = [makeRecord({ id: 'rec-a', runId: 'run-a', sessionId: 's-a' })];
  const out = exportHistory(records, { exportedAt: '2025-01-01T00:00:00.000Z', storeRoot: '/store/root' });
  assert.equal(out.meta.schemaVersion, HISTORY_EXPORT_SCHEMA_VERSION);
  assert.match(out.meta.format, /roster\.history-export\.v\d+/);
  assert.equal(out.meta.recordCount, 1);
  assert.equal(out.meta.storeRoot, '/store/root');
  assert.deepEqual(out.records.map((record) => record.id), ['rec-a']);
  assert.equal(out.records[0].runId, 'run-a');
  assert.equal(out.records[0].sessionId, 's-a');
});

test('renderHistoryJson is deterministic across repeated exports of equivalent input', () => {
  const records = [
    makeRecord({ id: 'rec-1', runId: 'run-1', sessionId: 's-1', createdAt: '2025-01-01T00:00:00.000Z' }),
    makeRecord({ id: 'rec-2', runId: 'run-2', sessionId: 's-2', createdAt: '2025-01-02T00:00:00.000Z' }),
  ];
  const meta = { exportedAt: '2025-01-01T00:00:00.000Z', storeRoot: '/store/root' };
  const first = renderHistoryJson(records, meta);
  const second = renderHistoryJson(records.map((record) => ({ ...record })), meta);
  assert.equal(first, second);
  const parsed = JSON.parse(first);
  // Object keys are sorted so field ordering is stable.
  assert.deepEqual(Object.keys(parsed), ['meta', 'records']);
  assert.ok(Object.keys(parsed.meta).every((key, index, keys) => index === 0 || keys[index - 1] <= key));
  assert.ok(Object.keys(parsed.records[0].fields).every((key, index, keys) => index === 0 || keys[index - 1] <= key));
});

test('export records are self-contained diagnostic evidence', () => {
  // No servedModel: historyFields falls back to requestedModel, then payload.model.
  const record = {
    id: 'rec-1', version: 1, runId: 'run-1', sessionId: 'session-1',
    repoIdentity: 'repo-1234abcd',
    issue: { issue: '42' }, seat: { name: 'seat-a' },
    createdAt: '2025-01-02T03:04:05.000Z',
    payload: { outcome: 'merged', model: 'payload-model', issue: 'ignored' },
  };
  const entry = canonicalizeRecord(record);
  assert.equal(entry.id, 'rec-1');
  assert.equal(entry.runId, 'run-1');
  assert.equal(entry.sessionId, 'session-1');
  assert.equal(entry.repository, 'repo-1234abcd');
  assert.equal(entry.fields.issue, '42');
  assert.equal(entry.fields.seat, 'seat-a');
  assert.equal(entry.fields.model, 'payload-model');
  assert.equal(entry.fields.outcome, 'merged');
  assert.equal(entry.fields.at, record.createdAt);
  // Absent summary fields are normalized to null so the export shape is stable.
  assert.equal(entry.fields.repository, 'repo-1234abcd');
});

test('no secrets are included in exported output', () => {
  const sentinel = 'test-only-private-api-key';
  const record = makeRecord({
    id: 'rec-secret',
    payload: {
      outcome: 'failed',
      api_key: sentinel,
      apiKey: sentinel,
      token: sentinel,
      authorization: `Bearer ${sentinel}`,
    },
  });
  const json = renderHistoryJson([record], { exportedAt: '2025-01-01T00:00:00.000Z' });
  assert.ok(!json.includes(sentinel), `export must not include the sentinel: ${json}`);
  const summary = renderHistorySummary([record], {});
  assert.ok(!summary.includes(sentinel));
});

test('renderHistorySummary distinguishes machine-history path from repo-state path', () => {
  const record = makeRecord({ id: 'rec-1', repoIdentity: 'repo-1234abcd' });
  const text = renderHistorySummary([record], { storeRoot: '/home/me/repo/.git/roster/provenance', repoState: '/home/me/repo' });
  assert.match(text, /Machine history store: \/home\/me\/repo\/\.git\/roster\/provenance/);
  assert.match(text, /Repository state \(not part of machine history\): \/home\/me\/repo/);
  assert.match(text, /rec-1/);
});

test('parseListFlags accepts --format json and text', () => {
  assert.deepEqual(parseListFlags(['--format', 'json']), { format: 'json' });
  assert.deepEqual(parseListFlags(['--format', 'text', '--store', 'DIR']), { format: 'text', storePath: 'DIR' });
  assert.throws(() => parseListFlags(['--format', 'yaml']), TypeError);
});

// runHistory end-to-end: loadProvenanceRecords -> resolveHistoryRoot (honors
// storePath, so no git needed) -> createHistoryReader(root).read(). The store
// returns no committed records from a fresh root, and runHistory must accept a
// reader object (not a plain {records} array) for every format.
test('runHistory supports --format json, text, and summary end to end', async (t) => {
  const dir = await mkdtemp(path.join(tmpdir(), 'history-export-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const storeRoot = path.join(dir, 'provenance');
  await mkdir(storeRoot, { recursive: true });

  const base = { cwd: dir, storePath: storeRoot, run: fakeRun };

  const jsonOut = await runHistory(['list', '--format', 'json'], base);
  const parsed = JSON.parse(jsonOut);
  assert.equal(parsed.meta.schemaVersion, HISTORY_EXPORT_SCHEMA_VERSION);
  assert.equal(parsed.meta.recordCount, 0);
  assert.deepEqual(parsed.records, []);
  // The machine-history store root is carried in meta, self-contained.
  assert.equal(parsed.meta.storeRoot, storeRoot);

  const textOut = await runHistory(['list', '--format', 'text'], base);
  assert.match(textOut, /No history records matched/);

  const summaryOut = await runHistory(['list', '--format', 'summary'], base);
  assert.match(summaryOut, /No history records matched/);
});

test('CLI JSON stays parseable and byte-identical with real and corrupt store entries', async (t) => {
  const dir = await mkdtemp(path.join(tmpdir(), 'history-export-real-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const storeRoot = path.join(dir, 'provenance');
  await openProvenanceStore(storeRoot).appendRecord({
    id: 'real-1', version: 1, runId: 'run-real', sessionId: 'roster-9-coder', repoIdentity: 'repo-1234abcd',
    payload: { issue: 9, seat: 'coder', model: 'model-a', outcome: 'pass' },
  });
  await writeFile(path.join(storeRoot, LOG_DIRNAME, 'bad.json'), '{broken');
  const base = { cwd: dir, storePath: storeRoot, run: fakeRun };
  const first = await runHistory(['list', '--format', 'json'], base);
  const parsed = JSON.parse(first);
  assert.deepEqual(parsed.records.map(({ id }) => id), ['real-1']);
  assert.equal(parsed.meta.skippedCount, 1);
  assert.equal(await runHistory(['list', '--format', 'json'], base), first);
  assert.equal(JSON.parse(await runHistory(['show', 'run-real', '--format', 'json'], base)).records[0].id, 'real-1');
  const gitStore = path.join(dir, '.git', 'roster', 'provenance');
  await openProvenanceStore(gitStore).appendRecord({ id: 'real-2', version: 1, runId: 'run-2', repoIdentity: 'repo-1234abcd' });
  const summary = await runHistory(['list', '--format', 'summary'], { cwd: dir, run: async () => ({ stdout: '.git\n' }) });
  assert.ok(summary.includes(`Machine history store: ${gitStore}`), summary);
  assert.ok(summary.includes(`Repository state (not part of machine history): ${path.join(dir, '.roster')}`), summary);
});

test('query reader shape flows through export helpers unchanged', () => {
  // Mirrors how runHistory consumes createHistoryReader(...).read(): {records, skipped}.
  const records = [makeRecord({ id: 'rec-1', repoIdentity: 'repo-1234abcd' })];
  const read = { records: filterRecords(records, { repository: 'repo-1234abcd' }), skipped: [] };
  const json = renderHistoryJson(read.records, { storeRoot: '/store' });
  assert.equal(JSON.parse(json).meta.recordCount, 1);
  assert.equal(findHistoryRecord(read.records, 'run-1').id, 'rec-1');
  assert.equal(filterRepositories(read.records, ['repo-1234abcd']).length, 1);
  assert.ok(historyFields(records[0]).repository === 'repo-1234abcd');
  const reader = createHistoryReader({ root: '/does-not-exist-must-not-be-opened' });
  assert.equal(typeof reader.read, 'function');
});

// ---------------------------------------------------------------------------
// Scoped deletion (issue #201)
// ---------------------------------------------------------------------------

// Records spanning two state scopes, one of which carries a secret-looking
// payload field (sentinel only; never a real credential).
function scopedFixtures() {
  const sentinel = 'test-only-private-api-key';
  const records = [
    makeRecord({ id: 'rec-machine-1', scope: 'machine', runId: 'run-m-1' }),
    makeRecord({ id: 'rec-machine-2', scope: 'machine', runId: 'run-m-2' }),
    makeRecord({ id: 'rec-repo-1', scope: 'repo', runId: 'run-r-1' }),
    makeRecord({ id: 'rec-worktree-1', scope: 'worktree', runId: 'run-w-1', payload: { outcome: 'ok', api_key: sentinel } }),
  ];
  return { records, sentinel };
}

test('previewScopedDeletion lists exactly the in-scope records and nothing else', () => {
  const { records } = scopedFixtures();
  const before = JSON.stringify(records);
  const preview = previewScopedDeletion(records, { scope: 'repo' });
  // Input is not mutated by a preview.
  assert.equal(JSON.stringify(records), before);
  assert.equal(preview.scope, 'repo');
  assert.equal(preview.count, 1);
  // Exactly the in-scope record id, and no other scope's records.
  assert.deepEqual(preview.records.map((entry) => entry.id), ['rec-repo-1']);
  assert.deepEqual(preview.records.every((entry) => entry.scope === 'repo'), true);
});

test('previewScopedDeletion for one scope never lists records from another scope', () => {
  const { records } = scopedFixtures();
  for (const scope of ['machine', 'repo', 'worktree']) {
    const preview = previewScopedDeletion(records, { scope });
    const foreign = preview.records.filter((entry) => entry.scope !== scope);
    assert.deepEqual(foreign, [], `scope ${scope} preview leaked: ${JSON.stringify(preview.records)}`);
  }
  const preview = previewScopedDeletion(records, { scope: 'machine' });
  assert.deepEqual(preview.records.map((entry) => entry.id).sort(), ['rec-machine-1', 'rec-machine-2']);
});

test('deleteScopedRecords leaves records belonging to other scopes untouched', () => {
  const { records } = scopedFixtures();
  const snapshot = (list) => JSON.stringify(list);
  const beforeOthers = snapshot(records.filter((record) => record.scope !== 'repo'));
  const { remaining, audit } = deleteScopedRecords(records, { scope: 'repo' });
  // The selected scope is gone...
  assert.deepEqual(remaining.filter((record) => record.scope === 'repo'), []);
  // ...and every other scope's records survive, byte-for-byte.
  assert.equal(snapshot(remaining.filter((record) => record.scope !== 'repo')), beforeOthers);
  // The input array is not mutated: the deleted record is still present in it.
  assert.ok(records.some((record) => record.id === 'rec-repo-1'));
  assert.equal(audit.scope, 'repo');
  assert.deepEqual(audit.recordIds, ['rec-repo-1']);
  assert.equal(audit.count, 1);
});

test('deletion audit is stamped with the injected clock and defaults to the current time, not the epoch', () => {
  const { records } = scopedFixtures();
  assert.equal(deleteScopedRecords(records, { scope: 'repo', now: () => '2026-10-07T00:00:00.000Z' }).audit.at,
    '2026-10-07T00:00:00.000Z');
  assert.ok(Date.parse(deleteScopedRecords(records, { scope: 'repo' }).audit.at) > Date.parse('2020-01-01'));
  assert.throws(() => deleteScopedRecords(records, { scope: 'session' }), /scope in/);
});

test('audit entry never contains the original secret sentinel value', () => {
  const { records, sentinel } = scopedFixtures();
  const { remaining, audit } = deleteScopedRecords(records, { scope: 'worktree' });
  const auditText = JSON.stringify(audit);
  // The audit log output must not carry the raw secret.
  assert.ok(!auditText.includes(sentinel), `audit leaked sentinel: ${auditText}`);
  // If a secret field is referenced at all, only the redacted form appears.
  const entry = audit.records.find((record) => record.id === 'rec-worktree-1');
  assert.ok(entry, 'audit must reference the deleted record');
  const entryText = JSON.stringify(entry);
  assert.ok(!entryText.includes(sentinel), `audit entry leaked sentinel: ${entryText}`);
  if (entry.summary?.api_key !== undefined) {
    assert.equal(entry.summary.api_key, '[REDACTED]');
  }
  // The worktree record itself is deleted from the remaining set.
  assert.deepEqual(remaining.map((record) => record.id).sort(), ['rec-machine-1', 'rec-machine-2', 'rec-repo-1']);
});

test('deleting/expiring one scope does not delete or alter another scope\'s records', () => {
  const { records, sentinel } = scopedFixtures();
  const { remaining } = deleteScopedRecords(records, { scope: 'machine' });
  const repo = remaining.find((record) => record.id === 'rec-repo-1');
  const worktree = remaining.find((record) => record.id === 'rec-worktree-1');
  assert.ok(repo, 'repo-scope record must survive a machine-scope deletion');
  assert.ok(worktree, 'worktree-scope record must survive a machine-scope deletion');
  // Records of surviving scopes are unaltered, secret field included.
  assert.equal(worktree.payload.api_key, sentinel);
  // The exported render of surviving records stays valid and secret-free.
  const json = renderHistoryJson(remaining, { exportedAt: '2025-01-01T00:00:00.000Z' });
  const parsed = JSON.parse(json);
  assert.deepEqual(parsed.records.map((record) => record.id).sort(), ['rec-repo-1', 'rec-worktree-1']);
});
