import test from 'node:test';
import assert from 'node:assert/strict';
import {
  SCHEMA_VERSION,
  UNKNOWN_METRIC,
  KNOWN_METRICS,
  isKnownMetric,
  createRecord,
} from '../src/lib/provenance-schema.mjs';

const NOW = 1735689600000; // 2025-01-01T00:00:00Z

test('SCHEMA_VERSION is a semver-shaped string', () => {
  // App code under test: createRecord stamps SCHEMA_VERSION onto every record,
  // so the constant's shape is asserted through the record it produces and a
  // regression that stops stamping the version fails here.
  const record = createRecord({ run: { id: 'run-schema' } }, NOW);
  assert.equal(record.schemaVersion, SCHEMA_VERSION);
  assert.equal(typeof record.schemaVersion, 'string');
  assert.match(record.schemaVersion, /^\d+\.\d+\.\d+$/);
});

test('createRecord stamps SCHEMA_VERSION and isKnownMetric guards metric fields', () => {
  // App code under test: createRecord reads SCHEMA_VERSION; isKnownMetric guards the metrics map.
  const record = createRecord(
    { metrics: { duration_ms: 7, 'not-a-metric': 1 } },
    NOW,
  );
  assert.equal(record.schemaVersion, SCHEMA_VERSION);
  assert.ok(isKnownMetric(Object.keys(record.metrics)[0]));
  assert.ok(!isKnownMetric('not-a-metric'));
});

test('isKnownMetric identifies known metric fields only', () => {
  for (const name of KNOWN_METRICS) assert.ok(isKnownMetric(name), name);
  assert.ok(!isKnownMetric('made-up-metric'));
  assert.ok(!isKnownMetric(42));
  assert.ok(!isKnownMetric(undefined));
});

test('createRecord returns a record with all identity fields', () => {
  const record = createRecord({
    run: { id: 'run-1' },
    session: { id: 'session-1' },
    repository: { remote: 'repo', commit: 'abc123' },
    issue: { issue: '#195', task: 'history-schema' },
    seat: { name: 'seat-a' },
    route: { name: 'route-x' },
    requestedModel: 'model-large',
    servedModel: 'model-large-2025-01-01',
    startedAt: NOW - 60000,
    endedAt: NOW,
    outcome: 'succeeded',
    tools: { name: 'tool-a', version: '2.0.0' },
    metrics: { duration_ms: 1200, cost_usd: 0.01 },
  }, NOW);

  assert.equal(record.schemaVersion, SCHEMA_VERSION);
  assert.equal(record.runId, 'run-1');
  assert.equal(record.sessionId, 'session-1');
  assert.deepEqual(record.repository, { remote: 'repo', commit: 'abc123' });
  assert.deepEqual(record.issue, { issue: '#195', task: 'history-schema' });
  assert.deepEqual(record.seat, { name: 'seat-a' });
  assert.deepEqual(record.route, { name: 'route-x' });
  assert.equal(record.requestedModel, 'model-large');
  assert.equal(record.servedModel, 'model-large-2025-01-01');
  assert.equal(record.startedAt, NOW - 60000);
  assert.equal(record.endedAt, NOW);
  assert.equal(record.outcome, 'succeeded');
  assert.deepEqual(record.tools, { name: 'tool-a', version: '2.0.0' });
  assert.equal(record.createdAt, NOW);
});

test('createRecord is immutable (frozen)', () => {
  const record = createRecord({ run: { id: 'run-1' } }, NOW);
  assert.ok(Object.isFrozen(record));
  assert.ok(Object.isFrozen(record.metrics));
  assert.throws(() => {
    'use strict';
    record.runId = 'mutated';
  });
});

test('unknown and missing metrics are marked with the unknown sentinel, never 0 or null', () => {
  const record = createRecord(
    { metrics: { duration_ms: 5, 'made-up-metric': 99 } },
    NOW,
  );
  assert.equal(record.metrics.duration_ms, 5);
  assert.equal(record.metrics['made-up-metric'], UNKNOWN_METRIC);
  assert.equal(record.metrics.cost_usd, UNKNOWN_METRIC);
  assert.equal(record.metrics.tokens_prompt, UNKNOWN_METRIC);
  for (const value of Object.values(record.metrics)) {
    assert.notEqual(value, 0);
    assert.notEqual(value, null);
    assert.notEqual(value, undefined);
  }
});

test('invalid known metric values are marked unknown', () => {
  const record = createRecord(
    { metrics: { duration_ms: 'fast', cost_usd: NaN } },
    NOW,
  );
  assert.equal(record.metrics.duration_ms, UNKNOWN_METRIC);
  assert.equal(record.metrics.cost_usd, UNKNOWN_METRIC);
});

test('missing ids fall back to the unknown sentinel', () => {
  const record = createRecord({}, NOW);
  assert.equal(record.runId, UNKNOWN_METRIC);
  assert.equal(record.sessionId, UNKNOWN_METRIC);
  assert.equal(record.outcome, '');
});

test('evidence and metrics sub-objects stay frozen with the record', () => {
  const record = createRecord({ evidence: { log: 'line' } }, NOW);
  assert.ok(Object.isFrozen(record.evidence));
});
