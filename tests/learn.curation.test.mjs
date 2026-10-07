import test from 'node:test';
import assert from 'node:assert/strict';
import { curateMemory, curatedRecordId } from '../src/lib/learn.mjs';
import { compactMemory } from '../src/runtime/memory.mjs';
import { storeRecordId } from '../src/lib/provenance-api.mjs';

// Realistic repository identity hash (algorithm-hex format).
const REPO_ID = 'repo-a3f2b1c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2';

test('curateMemory: empty input returns empty curated memory', () => {
  const result = curateMemory([]);
  assert.deepEqual(result, { records: [], pruned: 0 });
});

test('curateMemory: curated entries link back to source record IDs from compaction output', () => {
  const sourceRecords = [
    { runId: 'run-1', sessionId: 's1', event: 'completed', key: 'model', value: 'gpt-4' },
    { runId: 'run-2', sessionId: 's2', event: 'completed', key: 'model', value: 'gpt-4o' },
  ];
  const { records: compacted } = compactMemory(sourceRecords, { limit: 10, identity: REPO_ID });
  const { records: curated } = curateMemory(compacted, { identity: REPO_ID });

  assert.equal(curated.length, 1);
  const entry = curated[0];

  assert.ok(Array.isArray(entry.sourceRecordIds), 'sourceRecordIds must be an array');
  assert.ok(entry.sourceRecordIds.length >= 1, 'sourceRecordIds must not be empty');

  for (const compactionRecord of compacted) {
    if (compactionRecord.id) {
      assert.ok(entry.compactionIds.includes(compactionRecord.id),
        `curated entry must reference compaction record ${compactionRecord.id}`);
    }
  }

  assert.ok(entry.sourceRunIds.includes('run-1'));
  assert.ok(entry.sourceRunIds.includes('run-2'));
});

test('curateMemory: conflicting values for same key — newest retained, older pruned, provenance union preserved', () => {
  const sourceRecords = [
    { runId: 'run-old', sessionId: 's1', event: 'completed', key: 'setting', value: 'old-value', timestamp: '2024-01-01T00:00:00Z' },
    { runId: 'run-new', sessionId: 's2', event: 'completed', key: 'setting', value: 'new-value', timestamp: '2024-06-01T00:00:00Z' },
  ];
  const { records: compacted } = compactMemory(sourceRecords, { limit: 10, identity: REPO_ID });
  const { records: curated, pruned } = curateMemory(compacted, { identity: REPO_ID });

  assert.equal(curated.length, 1);
  assert.equal(pruned, 1);
  const entry = curated[0];

  assert.equal(entry.value, 'new-value');
  assert.equal(entry.runId, 'run-new');

  assert.ok(entry.sourceRunIds.includes('run-old'),
    'pruned entry provenance must still be preserved in sourceRunIds');
  assert.ok(entry.sourceRunIds.includes('run-new'),
    'retained entry provenance must be in sourceRunIds');
});

test('curateMemory: duplicate keys with identical timestamps — deterministic tie-break by run ID', () => {
  const sourceRecords = [
    { runId: 'run-b', sessionId: 's1', event: 'completed', key: 'k', value: 'v', timestamp: '2024-01-01T00:00:00Z' },
    { runId: 'run-a', sessionId: 's2', event: 'completed', key: 'k', value: 'v', timestamp: '2024-01-01T00:00:00Z' },
  ];
  const { records: compacted } = compactMemory(sourceRecords, { limit: 10, identity: REPO_ID });
  const { records: curated } = curateMemory(compacted, { identity: REPO_ID });

  assert.equal(curated.length, 1);
  const entry = curated[0];

  assert.ok(entry.sourceRunIds.includes('run-a'));
  assert.ok(entry.sourceRunIds.includes('run-b'));
  // Deterministic tie-break: higher runId wins ('run-b' > 'run-a')
  assert.equal(entry.runId, 'run-b');
});

test('curateMemory: stale entries are pruned without dropping most recent non-conflicting source record IDs', () => {
  const sourceRecords = [
    { runId: 'run-1', sessionId: 's1', event: 'completed', key: 'note', value: 'first', timestamp: '2024-01-01T00:00:00Z' },
    { runId: 'run-2', sessionId: 's2', event: 'completed', key: 'note', value: 'first', timestamp: '2024-01-02T00:00:00Z' },
    { runId: 'run-3', sessionId: 's3', event: 'completed', key: 'note', value: 'second', timestamp: '2024-01-03T00:00:00Z' },
  ];
  const { records: compacted } = compactMemory(sourceRecords, { limit: 10, identity: REPO_ID });
  const { records: curated, pruned } = curateMemory(compacted, { identity: REPO_ID });

  assert.equal(curated.length, 1);
  assert.ok(pruned >= 1, 'stale/conflicting entries should be pruned');
  const entry = curated[0];

  assert.equal(entry.runId, 'run-3');
  assert.equal(entry.value, 'second');

  assert.ok(entry.sourceRunIds.includes('run-1'));
  assert.ok(entry.sourceRunIds.includes('run-2'));
  assert.ok(entry.sourceRunIds.includes('run-3'));
});

test('curateMemory: different keys produce separate curated entries', () => {
  const sourceRecords = [
    { runId: 'run-1', sessionId: 's1', event: 'completed', key: 'model', value: 'gpt-4' },
    { runId: 'run-2', sessionId: 's2', event: 'completed', key: 'effort', value: 'h' },
  ];
  const { records: compacted } = compactMemory(sourceRecords, { limit: 10, identity: REPO_ID });
  const { records: curated } = curateMemory(compacted, { identity: REPO_ID });

  assert.equal(curated.length, 2);
  const keys = curated.map((r) => r.key).sort();
  assert.deepEqual(keys, ['effort', 'model']);
});

test('curateMemory: records without key field use default key', () => {
  const sourceRecords = [
    { runId: 'run-1', sessionId: 's1', event: 'completed', value: 'v1' },
    { runId: 'run-2', sessionId: 's2', event: 'completed', value: 'v2' },
  ];
  const { records: compacted } = compactMemory(sourceRecords, { limit: 10, identity: REPO_ID });
  const { records: curated } = curateMemory(compacted, { identity: REPO_ID });

  assert.equal(curated.length, 1);
  assert.equal(curated[0].key, 'default');
  assert.ok(curated[0].sourceRunIds.includes('run-1'));
  assert.ok(curated[0].sourceRunIds.includes('run-2'));
});

test('curatedRecordId: derives stable curated memory record IDs', () => {
  const compactionRecord = { runId: 'r1', sessionId: 's1', key: 'test-key', id: 'abc123' };
  const id = curatedRecordId(compactionRecord, { identity: REPO_ID });
  assert.equal(typeof id, 'string');
  assert.equal(id.length, 64);
  assert.equal(id, curatedRecordId(compactionRecord, { identity: REPO_ID }));
});

test('end-to-end: compactMemory → curateMemory preserves full provenance chain', () => {
  const sourceRecords = [
    { runId: 'run-orig-1', sessionId: 'sess-a', event: 'completed', key: 'task', value: 'issue-201' },
    { runId: 'run-orig-2', sessionId: 'sess-b', event: 'completed', key: 'task', value: 'issue-202' },
    { runId: 'run-orig-3', sessionId: 'sess-c', event: 'completed', key: 'task', value: 'issue-202' },
  ];
  const { records: compacted } = compactMemory(sourceRecords, { limit: 10, identity: REPO_ID });
  assert.equal(compacted.length, 3);

  for (const record of compacted) {
    assert.ok(Array.isArray(record.sourceRunIds));
    assert.ok(record.sourceRunIds.length >= 1);
  }

  const { records: curated } = curateMemory(compacted, { identity: REPO_ID });
  assert.equal(curated.length, 1);
  const entry = curated[0];

  for (const record of compacted) {
    assert.ok(entry.compactionIds.includes(record.id));
  }
  assert.ok(entry.sourceRunIds.includes('run-orig-1'));
  assert.ok(entry.sourceRunIds.includes('run-orig-2'));
  assert.ok(entry.sourceRunIds.includes('run-orig-3'));
  assert.ok(entry.sourceRecordIds.length >= 3);
});

test('storeRecordId: compaction and curated IDs use the same identity for cross-referencing', () => {
  const id = storeRecordId(REPO_ID, 'run1', 'sess1', 'completed', 'raw-history');
  assert.equal(typeof id, 'string');
  assert.equal(id.length, 64);
  // Same identity + same inputs → same ID (deterministic cross-referencing)
  assert.equal(id, storeRecordId(REPO_ID, 'run1', 'sess1', 'completed', 'raw-history'));
  // Different identity → different ID (no cross-repo collision)
  const otherId = storeRecordId('repo-b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2', 'run1', 'sess1', 'completed', 'raw-history');
  assert.notEqual(id, otherId);
});
