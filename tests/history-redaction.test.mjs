import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { ProvenanceStore } from '../src/lib/provenance-api.mjs';
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';

import {
  createRedactionConfig,
  createProvenance,
  computeProvenanceDigest,
  PROVENANCE_SCHEMA_VERSION,
  PROVENANCE_SCHEMA,
  REDACTION_MARKER,
  HOME_MARKER,
  redactString,
  redactRecord,
  validateProvenance,
  validateProvenanceRecords,
  scanMalformedRecords,
} from '../src/lib/history-redaction.mjs';

import {
  exportHistory,
  renderHistoryJson,
  renderHistorySummary,
  HISTORY_EXPORT_SCHEMA_VERSION,
} from '../src/lib/history-export.mjs';

// Obvious non-credential sentinels only.
const FAKE_API_KEY = 'test-only-private-api-key';
const FAKE_KEY_PATH = '/home/tester/.ssh/test-only-private-key';

// Build a history record with valid, versioned provenance.
function makeRecord(overrides = {}) {
  const record = {
    id: 'sess-001',
    version: 1,
    runId: 'run-001',
    sessionId: 'sess-001',
    repoIdentity: 'acme/test-repo',
    issue: { issue: '#42' },
    seat: { name: 'alpha' },
    servedModel: 'test-model',
    createdAt: '2024-01-01T00:00:00.000Z',
    ...overrides,
  };
  return { ...record, provenance: createProvenance(record) };
}

test('redacts configured API key from strings (sentinel via env)', () => {
  const config = createRedactionConfig({ env: { API_KEY: FAKE_API_KEY } });
  const line = `Authorization: Bearer ${FAKE_API_KEY}`;
  const out = redactString(line, config);
  assert.equal(out.includes(FAKE_API_KEY), false);
  assert.ok(out.includes(REDACTION_MARKER));
});

test('redacts configured private key path and value', () => {
  const config = createRedactionConfig({
    values: [FAKE_API_KEY],
    paths: [FAKE_KEY_PATH],
  });
  const text = `loaded ${FAKE_KEY_PATH} using ${FAKE_API_KEY}`;
  const out = redactString(text, config);
  assert.equal(out.includes(FAKE_KEY_PATH), false);
  assert.equal(out.includes(FAKE_API_KEY), false);
  assert.equal(out, `loaded ${REDACTION_MARKER} using ${REDACTION_MARKER}`);
});

test('redactRecord masks secrets in nested fields and keeps unknown metrics absent/null', () => {
  const config = createRedactionConfig({ values: [FAKE_API_KEY], paths: [FAKE_KEY_PATH] });
  const record = {
    env: { API_KEY: FAKE_API_KEY, PATH: '/usr/bin' },
    notes: `key file at ${FAKE_KEY_PATH}`,
    metrics: { unknownMetric: null },
  };
  const out = redactRecord(record, config);
  const json = JSON.stringify(out);
  assert.equal(json.includes(FAKE_API_KEY), false);
  assert.equal(json.includes(FAKE_KEY_PATH), false);
  assert.equal(out.env.API_KEY, REDACTION_MARKER);
  assert.equal(out.env.PATH, '/usr/bin');
  // Unknown metrics stay null and are never synthesized.
  assert.equal(out.metrics.unknownMetric, null);
  assert.ok('unknownMetric' in out.metrics);
});

test('home directory is masked with ~ unless the record opts in', () => {
  const config = createRedactionConfig({ home: '/home/tester' });
  const plain = redactRecord({ cwd: '/home/tester/.cache' }, config);
  assert.equal(plain.cwd, `${HOME_MARKER}/.cache`);
  assert.equal(plain.cwd.includes('/home/tester'), false);

  const optIn = redactRecord({ cwd: '/home/tester/.cache', keepHome: true }, config);
  assert.equal(optIn.cwd, '/home/tester/.cache');
});

test('valid versioned provenance with correct digest passes', () => {
  const record = makeRecord();
  const result = validateProvenance(record);
  assert.equal(result.ok, true);
  assert.equal(record.provenance.version, PROVENANCE_SCHEMA_VERSION);
  assert.equal(record.provenance.digest, computeProvenanceDigest(record));
});

test('tampered provenance is rejected', () => {
  const record = makeRecord();
  record.provenance.digest = '0'.repeat(64);
  const result = validateProvenance(record);
  assert.equal(result.ok, false);
});

test('field change after signing is rejected (tamper detection)', () => {
  const record = makeRecord();
  record.servedModel = 'other-model';
  assert.equal(validateProvenance(record).ok, false);
});

test('missing integrity on a provenance-claimed record is rejected', () => {
  // A null block is a present-but-malformed block: it claims provenance but
  // provides no integrity, so it must fail.
  assert.equal(validateProvenance({ ...makeRecord(), provenance: null }).ok, false);
  // A record with no provenance block still needs a valid stored id and version.
  const { provenance, ...bare } = makeRecord();
  assert.deepEqual(validateProvenance(bare), { ok: true, integrity: 'unbound' });
  const { version, ...unversioned } = bare;
  assert.equal(validateProvenance(unversioned).ok, false);
});

test('live typed-API records verify through their derived id and reject tampering', async (t) => {
  const repoRoot = await mkdtemp(path.join(tmpdir(), 'history-integrity-'));
  t.after(() => rm(repoRoot, { recursive: true, force: true }));
  const identity = `sha256-${'a'.repeat(64)}`;
  const store = new ProvenanceStore({ root: path.join(repoRoot, 'provenance'), resolveIdentity: () => identity });
  const written = await store.record({ runId: 'run-live', sessionId: 'roster-9-coder', event: 'session',
    payload: { issue: 9, seat: 'coder' } });
  const { records } = await openProvenanceStore(path.join(repoRoot, 'provenance')).readAll();
  const live = records.find(({ id }) => id === written.id);
  assert.deepEqual(validateProvenance(live), { ok: true, integrity: 'verified' });
  assert.equal(JSON.parse(renderHistoryJson([live])).records[0].integrity, 'verified');
  for (const [field, value] of [['runId', 'run-forged'], ['sessionId', 'roster-9-reviewer'],
    ['event', 'end'], ['repoIdentity', `sha256-${'b'.repeat(64)}`], ['section', 'curated-memory']]) {
    const forged = { ...live, [field]: value };
    assert.equal(validateProvenance(forged).ok, false, `${field} tamper must fail`);
    assert.throws(() => exportHistory([forged]), /Provenance validation failed/);
  }
  assert.match(validateProvenance({ ...live, section: 'other' }).error, /unknown provenance section/);
});

test('wrong provenance version is rejected', () => {
  const record = makeRecord();
  record.provenance = { ...record.provenance, version: 99 };
  const result = validateProvenance(record);
  assert.equal(result.ok, false);
  assert.match(result.error, /version/);
});

test('malformed provenance block is rejected', () => {
  assert.equal(validateProvenance({ ...makeRecord(), provenance: 'nope' }).ok, false);
  assert.equal(validateProvenance({ ...makeRecord(), provenance: null }).ok, false);
  assert.equal(validateProvenance(null).ok, false);
});

test('validateProvenanceRecords reports the first failing index', () => {
  const good = makeRecord();
  const bad = makeRecord({ id: 'sess-002' });
  bad.provenance = { ...bad.provenance, digest: '1'.repeat(64) };
  const result = validateProvenanceRecords([good, bad]);
  assert.equal(result.ok, false);
  assert.equal(result.index, 1);
});

test('validateProvenanceRecords quarantines schema-incompatible records without touching valid siblings', () => {
  const good1 = makeRecord();
  const good2 = makeRecord({ id: 'sess-002' });
  // A non-object entry and a version-drifted record are schema-incompatible.
  const bad0 = 'not-a-record';
  const bad3 = { ...makeRecord({ id: 'sess-003' }), version: 99 };
  const snapshot = [bad0, good1, good2, bad3].map((r) => JSON.parse(JSON.stringify(r)));

  const result = validateProvenanceRecords([bad0, good1, good2, bad3]);
  assert.equal(result.ok, false);
  // The first offender is surfaced for fail-closed exports.
  assert.equal(result.index, 0);
  assert.equal(typeof result.error, 'string');
  // Every offender is reported with its index and error, in order.
  assert.deepEqual(
    result.quarantine.map((q) => q.index),
    [0, 3],
  );
  for (const q of result.quarantine) assert.equal(typeof q.error, 'string');
  // Valid siblings are not dropped or mutated by the scan.
  assert.deepEqual(snapshot[1], good1);
  assert.deepEqual(snapshot[2], good2);
  assert.equal(validateProvenance(good1).ok, true);
  assert.equal(validateProvenance(good2).ok, true);
});

test('exportHistory applies redaction and fails closed on invalid provenance', () => {
  // Provenance is minted over the redacted bytes, so the exported record
  // carries verified provenance while still hiding the sentinels.
  const config = createRedactionConfig({ values: [FAKE_API_KEY], paths: [FAKE_KEY_PATH] });
  const redacted = redactRecord(
    {
      id: 'sess-001',
      version: 1,
      runId: 'run-001',
      sessionId: 'sess-001',
      repoIdentity: `acme/test-repo from ${FAKE_KEY_PATH}`,
      servedModel: FAKE_API_KEY,
      createdAt: '2024-01-01T00:00:00.000Z',
    },
    config,
  );
  const record = { ...redacted, provenance: createProvenance(redacted) };

  // The sentinel must be absent from every exported representation.
  const exported = exportHistory([record], { storeRoot: null });
  const json = renderHistoryJson([record]);
  const summary = renderHistorySummary([record]);
  for (const out of [JSON.stringify(exported), json, summary]) {
    assert.equal(out.includes(FAKE_API_KEY), false);
    assert.equal(out.includes(FAKE_KEY_PATH), false);
  }
  assert.ok(json.includes(`roster.history-export.v${HISTORY_EXPORT_SCHEMA_VERSION}`));

  // Fails closed before export: tampered digest.
  const tampered = makeRecord();
  tampered.provenance = { ...tampered.provenance, digest: '2'.repeat(64) };
  assert.throws(() => exportHistory([tampered]), /Provenance validation failed/);
  assert.throws(() => renderHistorySummary([tampered]), /Provenance validation failed/);

  // Fails closed: a null provenance block (claimed but no integrity).
  const nullProv = makeRecord();
  nullProv.provenance = null;
  assert.throws(() => exportHistory([nullProv]), /Provenance validation failed/);
});

test('renderHistoryJson remains deterministic for identical valid input', () => {
  const a = makeRecord();
  const b = makeRecord();
  assert.equal(renderHistoryJson([a]), renderHistoryJson([b]));
});

test('redactRecord redacts env-sourced secrets before persistence and leaves no live secret in output', () => {
  // The env secret feeds the app under test through a realistic secret
  // context (API_KEY env var) and the record carries it in a payload.
  const config = createRedactionConfig({ env: { API_KEY: FAKE_API_KEY } });
  const record = {
    id: 'sess-secret',
    version: 1,
    runId: 'run-001',
    sessionId: 'sess-001',
    payload: { api_key: FAKE_API_KEY, note: `deployed with ${FAKE_API_KEY}` },
  };
  const out = redactRecord(record, config);
  const json = JSON.stringify(out);
  assert.equal(json.includes(FAKE_API_KEY), false);
  assert.equal(typeof out, 'object');
  assert.notEqual(out, null);
  // The configured value is replaced with the deterministic marker.
  assert.ok(json.includes(REDACTION_MARKER));
});

test('scanMalformedRecords detects legacy malformed records and returns their indices', () => {
  const good = makeRecord();
  const good2 = makeRecord({ id: 'sess-002' });
  const records = [
    'legacy-text-line', // 0: non-object entry
    null,               // 1: non-object entry
    good,               // 2: valid
    { id: 'sess-x', version: 1, runId: 'run-x' }, // 3: missing required field (sessionId)
    { version: 1, runId: 'run-x', sessionId: 's-x' }, // 4: missing id
    { id: 'sess-y', version: 99, runId: 'run-y', sessionId: 's-y' }, // 5: incompatible version
    [1, 2, 3],          // 6: array entry
    good2,              // 7: valid
  ];
  const indices = scanMalformedRecords(records);
  assert.deepEqual(indices, [0, 1, 3, 4, 5, 6]);
  // Valid entries are untouched by the scan.
  assert.equal(validateProvenance(good).ok, true);
  assert.equal(validateProvenance(good2).ok, true);
  // A clean list yields no offenders.
  assert.deepEqual(scanMalformedRecords([good, good2]), []);
  // A non-array store fails closed instead of reporting nothing malformed.
  assert.throws(() => scanMalformedRecords('nope'), TypeError);
  assert.throws(() => scanMalformedRecords({ records: [] }), TypeError);
});
