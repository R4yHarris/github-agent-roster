import test from 'node:test';
import assert from 'node:assert/strict';
import { redactEvidence, redactRecord, secretMaterialLines } from '../src/lib/redaction.mjs';
import { createRecord } from '../src/lib/provenance-schema.mjs';

// Obvious non-credential sentinel; the tests assert it never survives redaction.
const SENTINEL = 'test-only-private-api-key';
const EMPTY_ENV = Object.freeze({});

test('redactEvidence replaces the sentinel key with an API key sentinel', () => {
  const out = redactEvidence(`api_key = ${SENTINEL}`, { env: EMPTY_ENV });
  assert.ok(!out.includes(SENTINEL), 'sentinel must not appear in redacted output');
  assert.ok(out.includes('[REDACTED:API_KEY]'));
});

test('redactRecord strips the sentinel from evidence passed into the app under test', () => {
  // The sentinel is passed INTO the app code (record input), not just inspected after the fact.
  const record = createRecord({
    run: { id: 'run-1' },
    sessionId: 'session-1',
    evidence: { log: `captured api_key=${SENTINEL} in the transcript` },
  }, 1735689600000);
  const redacted = redactRecord(record, { env: EMPTY_ENV });
  const serialized = JSON.stringify(redacted);
  assert.ok(!serialized.includes(SENTINEL), 'sentinel must be absent from redacted record output');
  assert.ok(serialized.includes('[REDACTED:API_KEY]'), 'evidence leaf is redacted to the API key sentinel');
});

test('redactEvidence replaces secret-like assignments', () => {
  const out = redactEvidence(`db_secret = hunter2xyz`, { env: EMPTY_ENV });
  assert.equal(out, 'db_secret: [REDACTED:SECRET]');
});

test('redactEvidence replaces private key blocks', () => {
  const block = `-----BEGIN RSA PRIVATE KEY-----\n${'A'.repeat(64)}\n-----END RSA PRIVATE KEY-----`;
  const out = redactEvidence(`preamble ${block} postamble`, { env: EMPTY_ENV });
  assert.ok(!out.includes('PRIVATE KEY-----\n'));
  assert.ok(out.includes('[REDACTED:PRIVATE_KEY]'));
  assert.ok(out.includes('preamble') && out.includes('postamble'));
});

test('redactEvidence keeps source around PEM headers that have no key body', () => {
  const body = 'MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7'.padEnd(64, 'A');
  const source = "const PEM_HEADER = '-----BEGIN TEST PRIVATE KEY-----';\n" +
    "const PEM_FOOTER = '-----END TEST PRIVATE KEY-----';\n" +
    "const prose = 'about -----BEGIN RSA PRIVATE KEY----- envelopes';\nconst tail = 1;";
  assert.equal(redactEvidence(source, { env: EMPTY_ENV }), source);
  const encrypted = `-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,ABCDEF\n\n${body}\nAB==\n` +
    '-----END RSA PRIVATE KEY-----\nafter';
  assert.equal(redactEvidence(encrypted, { env: EMPTY_ENV }), '[REDACTED:PRIVATE_KEY]\nafter');
  assert.equal(redactEvidence(`-----BEGIN PRIVATE KEY-----\n${body}\nrest`, { env: EMPTY_ENV }), '[REDACTED:PRIVATE_KEY]\nrest');
});

test('redactEvidence replaces values from env-sourced secrets', () => {
  const env = Object.freeze({ ROSTER_API_KEY: 'env-sourced-secret-value' });
  const out = redactEvidence('token is env-sourced-secret-value here', { env });
  assert.ok(!out.includes('env-sourced-secret-value'));
  assert.ok(out.includes('[redacted]'));
});

test('secretMaterialLines detects credential shapes but the test sentinel is not credential-shaped', () => {
  // Real credential shapes are still detected (behavior unchanged).
  const lines = secretMaterialLines(`line one\nsk-${'a'.repeat(30)}\nline three`, { env: EMPTY_ENV });
  assert.deepEqual(lines, [2]);
  // The sentinel is a non-credential value: redacted before persistence, but not
  // flagged as secret material by the detection scanner (no extra gate reasons).
  assert.deepEqual(secretMaterialLines(`line one\nkey = ${SENTINEL}\nline three`, { env: EMPTY_ENV }), []);
});

test('redactRecord applies redaction recursively to schema records', () => {
  const record = createRecord({
    run: { id: 'run-1' },
    session: { id: 'session-1' },
    outcome: 'failed',
    evidence: {
      log: `env dump: api_key=${SENTINEL} done`,
      nested: { note: 'a secret called deploy_secret = topsecret123 was set' },
      list: [`plain`, `api_key=${SENTINEL} again`],
    },
    metrics: { duration_ms: 5 },
  }, 1735689600000);

  const redacted = redactRecord(record, { env: EMPTY_ENV });
  const serialized = JSON.stringify(redacted);
  assert.ok(!serialized.includes(SENTINEL), 'sentinel must never appear in redacted record output');
  assert.ok(!serialized.includes('topsecret123'));
  assert.ok(serialized.includes('[REDACTED:API_KEY]'));
  assert.ok(serialized.includes('[REDACTED:SECRET]'));

  // Non-sensitive content is preserved; schema shape is unchanged.
  assert.equal(redacted.runId, 'run-1');
  assert.equal(redacted.sessionId, 'session-1');
  assert.equal(redacted.metrics.duration_ms, 5);
  assert.equal(redacted.evidence.list[0], 'plain');
});

test('redactRecord passes through primitives and arrays', () => {
  assert.equal(redactRecord(42), 42);
  assert.deepEqual(redactRecord([`x api_key=${SENTINEL} y`, null]), [`x api_key: [REDACTED:API_KEY] y`, null]);
});
