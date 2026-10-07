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
