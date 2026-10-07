import test from 'node:test';
import assert from 'node:assert/strict';
import {
  compactMemory,
  compactionRecordId,
  DEFAULT_COMPACTION_LIMIT,
  redactSecrets,
} from '../src/runtime/memory.mjs';
import {
  buildCompactionRecord,
  buildCompactionProvenance,
  storeRecordId,
  validateProvenanceRecord,
  buildProvenanceRecord,
} from '../src/lib/provenance-api.mjs';

// Realistic repository identity hash (algorithm-hex format).
const REPO_ID = 'repo-a3f2b1c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2';

test('compactMemory: empty input returns empty compaction list', () => {
  const result = compactMemory([]);
  assert.deepEqual(result, { records: [], dropped: 0, limit: DEFAULT_COMPACTION_LIMIT });
});

test('compactMemory: each compaction record includes source run IDs (provenance preserved)', () => {
  const records = [
    { runId: 'run-a', sessionId: 'sess-1', event: 'completed', summary: 'first' },
    { runId: 'run-b', sessionId: 'sess-2', event: 'completed', summary: 'second' },
    { runId: 'run-c', sessionId: 'sess-3', event: 'completed', summary: 'third' },
  ];
  const { records: compacted, dropped } = compactMemory(records, { limit: 10, identity: REPO_ID });
  assert.equal(dropped, 0);
  assert.equal(compacted.length, 3);

  for (const record of compacted) {
    assert.ok(Array.isArray(record.sourceRunIds), 'sourceRunIds must be an array');
    assert.ok(record.sourceRunIds.length >= 1, 'sourceRunIds must not be empty');
    assert.ok(record.sourceRunIds.includes(record.runId),
      `sourceRunIds must include the record's own runId ${record.runId}`);
    assert.ok(record.sourceSessionIds.includes(record.sessionId),
      'sourceSessionIds must include the record\'s own sessionId');
    assert.ok(typeof record.sourceRecordId === 'string' && record.sourceRecordId.length > 0,
      'sourceRecordId must be a non-empty string');
  }

  assert.ok(compacted[0].sourceRunIds.includes('run-a'));
  assert.ok(compacted[1].sourceRunIds.includes('run-b'));
  assert.ok(compacted[2].sourceRunIds.includes('run-c'));
});

test('compactMemory: bounded compaction drops oldest records and folds their provenance', () => {
  const records = Array.from({ length: 55 }, (_, i) => ({
    runId: `run-${i}`,
    sessionId: `sess-${i}`,
    event: 'completed',
    summary: `record ${i}`,
  }));
  const limit = 50;
  const { records: compacted, dropped } = compactMemory(records, { limit, identity: REPO_ID });
  assert.equal(dropped, 5);
  assert.equal(compacted.length, limit);

  const oldest = compacted[0];
  for (let i = 0; i < 5; i++) {
    assert.ok(oldest.sourceRunIds.includes(`run-${i}`),
      `oldest retained record must include dropped run-${i}`);
  }
  assert.ok(oldest.sourceRunIds.includes('run-5'));
});

test('compactMemory: records with sourceRunIds arrays merge them', () => {
  const records = [
    { runId: 'run-a', sessionId: 's1', event: 'completed', sourceRunIds: ['run-a', 'run-orig-1'] },
    { runId: 'run-b', sessionId: 's2', event: 'completed', sourceRunIds: ['run-b', 'run-orig-2'] },
  ];
  const { records: compacted } = compactMemory(records, { limit: 10, identity: REPO_ID });
  assert.ok(compacted[0].sourceRunIds.includes('run-a'));
  assert.ok(compacted[0].sourceRunIds.includes('run-orig-1'));
  assert.ok(compacted[1].sourceRunIds.includes('run-b'));
  assert.ok(compacted[1].sourceRunIds.includes('run-orig-2'));
});

test('compactMemory: missing provenance fields fall back to record runId/sessionId', () => {
  const records = [
    { summary: 'no runId or sessionId' },
  ];
  const { records: compacted } = compactMemory(records, { limit: 10, identity: REPO_ID });
  assert.equal(compacted.length, 1);
  assert.equal(compacted[0].runId, 'unknown');
  assert.equal(compacted[0].sessionId, 'unknown');
  assert.ok(Array.isArray(compacted[0].sourceRunIds));
});

test('compactMemory: limit below 1 is rejected rather than erasing memory', () => {
  const records = [
    { runId: 'run-a', sessionId: 's1', event: 'completed' },
    { runId: 'run-b', sessionId: 's2', event: 'completed' },
  ];
  assert.throws(() => compactMemory(records, { limit: 0 }), /positive safe integer/);
});

test('compaction ids stay distinct for same-run records and secrets are redacted from the compacted payload', () => {
  const sentinel = `ghp_${'A1b2C3d4E5'.repeat(4).slice(0, 36)}`;
  const records = [
    { runId: 'run-a', sessionId: 's1', event: 'completed', summary: 'first' },
    { runId: 'run-a', sessionId: 's1', event: 'completed', summary: 'second', api_key: sentinel },
  ];
  const { records: compacted } = compactMemory(records, { limit: 10, identity: REPO_ID });
  assert.notEqual(compacted[0].id, compacted[1].id);
  assert.notEqual(compacted[0].sourceRecordId, compacted[1].sourceRecordId);
  assert.equal(compacted[1].id, compactionRecordId(records[1], { identity: REPO_ID }));
  assert.ok(!JSON.stringify(compacted).includes(sentinel), 'compaction output must not carry the raw secret');
});

test('compactionRecordId: derives stable record IDs via storeRecordId', () => {
  const record = { runId: 'run-x', sessionId: 'sess-y', event: 'completed' };
  const id = compactionRecordId(record, { identity: REPO_ID });
  assert.equal(typeof id, 'string');
  assert.equal(id.length, 64);
  assert.equal(id, compactionRecordId(record, { identity: REPO_ID }));
  const otherId = compactionRecordId(record, { identity: 'repo-b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2' });
  assert.notEqual(id, otherId);
});

test('buildCompactionRecord: produces a record with id, sourceRunIds, sourceRecordId', () => {
  const source = { runId: 'r1', sessionId: 's1', event: 'completed', payload: {} };
  const record = buildCompactionRecord(source, { identity: REPO_ID });
  assert.ok(typeof record.id === 'string' && record.id.length === 64);
  assert.ok(record.sourceRunIds.includes('r1'));
  assert.ok(record.sourceSessionIds.includes('s1'));
  assert.ok(typeof record.sourceRecordId === 'string' && record.sourceRecordId.length === 64);
  assert.equal(record.section, 'compaction');
});

test('buildCompactionProvenance: creates provenance with source run IDs', () => {
  const source = { runId: 'r2', sessionId: 's2', event: 'session' };
  const { sourceRecordId, provenance } = buildCompactionProvenance(source, { identity: REPO_ID });
  assert.ok(typeof sourceRecordId === 'string' && sourceRecordId.length === 64);
  assert.equal(provenance.runId, 'r2');
  assert.equal(provenance.sessionId, 's2');
  assert.ok(provenance.payload.sourceRunIds.includes('r2'));
});

test('validateProvenanceRecord: still available and validates compaction provenance', () => {
  const record = { runId: 'r1', sessionId: 's1', event: 'completed' };
  const result = validateProvenanceRecord(record);
  assert.equal(result.runId, 'r1');
  assert.equal(result.sessionId, 's1');
  assert.equal(result.event, 'completed');

  assert.throws(() => validateProvenanceRecord({ runId: 'r1', sessionId: 's1', event: 'bad' }),
    /event must be one of/);
});

test('buildProvenanceRecord: still available for constructing provenance', () => {
  const record = buildProvenanceRecord(
    { runId: 'r9', sessionId: 's9', event: 'started', payload: { note: 'test' } },
    { redact: false },
  );
  assert.equal(record.runId, 'r9');
  assert.equal(record.sessionId, 's9');
  assert.equal(record.event, 'started');
  assert.equal(record.payload.note, 'test');
});

test('storeRecordId: still available and deterministic', () => {
  const id = storeRecordId(REPO_ID, 'run1', 'sess1', 'completed', 'raw-history');
  assert.equal(typeof id, 'string');
  assert.equal(id.length, 64);
  assert.equal(id, storeRecordId(REPO_ID, 'run1', 'sess1', 'completed', 'raw-history'));
  assert.notEqual(id, storeRecordId(REPO_ID, 'run2', 'sess1', 'completed', 'raw-history'));
});

test('redactSecrets: secrets in env are stripped from memory text before persistence', () => {
  const sentinel = 'test-only-private-api-key';
  const env = { ROSTER_API_KEY: sentinel };
  const text = `Connected with api_key=${sentinel} to endpoint`;
  const result = redactSecrets(text, { env });
  assert.ok(!result.includes(sentinel), 'sentinel must be redacted from output');
  assert.ok(result.includes('[redacted]'), 'output must contain redaction marker');
});
