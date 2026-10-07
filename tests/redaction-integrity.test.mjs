// Redaction integrity tests (parent #195): secret material from realistic
// contexts (env secrets, api_key assignments, PEM private keys) must never
// reach the persisted provenance record surface.
//
// The sentinel 'test-only-private-api-key' is a non-credential test marker:
// it is fed into the app code through a realistic secret context and we
// assert its absence from every persisted output. No sk-/ghp_/github_pat_
// values or PEM private-key blocks are written anywhere in this file; PEM
// coverage relies on the app's own pattern matching with placeholder
// base64 material.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { redactEvidence, redactRecord, secretMaterialLines } from '../src/lib/redaction.mjs';
import {
  buildProvenanceRecord,
  createProvenanceStore,
} from '../src/lib/provenance-api.mjs';
import { createRecord } from '../src/lib/provenance-schema.mjs';

// All sentinel values live in ROSTER_API_KEY: the typed API surface
// (buildProvenanceRecord / ProvenanceStore) redacts with its own env options
// that default to the real process env, so the only env secret value that can
// be guaranteed absent from its output is the value of the app's own
// apiKeyEnv variable. The full env-scan behavior (token/password/secret
// names) is verified directly against redactEvidence/redactRecord below,
// where the env options are injectable.
const SENTINEL = 'test-only-private-api-key';
const ENV = Object.freeze({ ROSTER_API_KEY: SENTINEL });

// Fixed clock so createdAt is deterministic and comparable across asserts.
const NOW = 1735689600000; // 2025-01-01T00:00:00Z

const REPO_IDENTITY = 'git-' + 'a'.repeat(64);

function assertNoSecretsPersisted(persistedText, label = 'persisted text') {
  assert.ok(
    !persistedText.includes(SENTINEL),
    `${label} must not contain the env api-key sentinel`,
  );
  assert.ok(
    !/-----BEGIN [A-Z ]*PRIVATE KEY-----/.test(persistedText),
    `${label} must not contain a PEM private-key block`,
  );
}

function textTree(value) {
  if (typeof value === 'string') return value;
  if (Array.isArray(value)) return value.map(textTree).join('\n');
  if (value && typeof value === 'object') {
    return Object.entries(value).map(([k, v]) => `${k}\n${textTree(v)}`).join('\n');
  }
  return String(value);
}

test('redactEvidence: env secret values, api_key assignments, and PEM blocks are removed', () => {
  // Realistic contexts: a mix of env secrets (key, token, password, secret)
  // leaked into evidence prose. All four values are distinct sentinels
  // delivered through realistic secret env names.
  const envLeak = Object.freeze({
    ROSTER_API_KEY: 'env-sentinel-key-abcdef123456',
    GITHUB_TOKEN: 'env-sentinel-token-abcdef123456',
    DB_PASSWORD: 'env-sentinel-password-abcdef123456',
    DEPLOY_SECRET: 'env-sentinel-secret-abcdef123456',
  });
  const leak = `deploy failed: key=${envLeak.ROSTER_API_KEY} token=${envLeak.GITHUB_TOKEN} password=${envLeak.DB_PASSWORD} secret=${envLeak.DEPLOY_SECRET}`;
  const envRedacted = redactEvidence(leak, { env: envLeak, apiKeyEnv: 'ROSTER_API_KEY' });
  for (const [name, value] of Object.entries(envLeak)) {
    assert.ok(!envRedacted.includes(value), `env ${name} value must be redacted`);
  }

  // Realistic context: an api_key assignment (the sentinel is the key value,
  // not a credential prefix, so the assignment pattern must catch it).
  const apiKeyAssign = `request headers: api_key: "${SENTINEL}" then x-api-key=${SENTINEL}`;
  const keyAssignRedacted = redactEvidence(apiKeyAssign, { env: {}, apiKeyEnv: 'ROSTER_API_KEY' });
  assert.ok(!keyAssignRedacted.includes(SENTINEL), 'api_key assignment value must be redacted');

  // Realistic context: generic secret assignment.
  const secretAssign = `config secret: ${envLeak.DEPLOY_SECRET}`;
  const secretRedacted = redactEvidence(secretAssign, { env: envLeak, apiKeyEnv: 'ROSTER_API_KEY' });
  assert.ok(!secretRedacted.includes(envLeak.DEPLOY_SECRET), 'secret assignment value must be redacted');

  // Realistic context: PEM private-key body (placeholder base64, not a real
  // key). The block, including its base64 body, must be replaced.
  const pemBody = 'PLACEHOLDERNOTAKEYPLACEHOLDERNOTAKEYPLACEHOLDERNOTAKEY';
  const pemText = `-----BEGIN RSA PRIVATE KEY-----\n${pemBody}\n-----END RSA PRIVATE KEY-----\nrest`;
  const pemRedacted = redactEvidence(pemText, { env: envLeak, apiKeyEnv: 'ROSTER_API_KEY' });
  assert.ok(!pemRedacted.includes('PRIVATE KEY-----'), 'PEM header must be redacted');
  assert.ok(!pemRedacted.includes(pemBody), 'PEM base64 body must be redacted');
  assert.ok(pemRedacted.includes('rest'), 'non-secret material survives redaction');

  // secretMaterialLines agrees: the PEM header line is flagged as secret
  // material, and the placeholder body is present in the source so the flag
  // is attributable to the PEM pattern.
  const lines = secretMaterialLines(pemText, { env: envLeak, apiKeyEnv: 'ROSTER_API_KEY' });
  assert.ok(lines.includes(1), 'the PEM header line must be flagged as secret material');
  assert.ok(pemText.includes(pemBody), 'placeholder key material is present in the source');
});

test('redactRecord: recursively redacts nested payload strings, preserves non-secret data', () => {
  const envLeak = Object.freeze({
    ROSTER_API_KEY: SENTINEL,
    GITHUB_TOKEN: 'env-sentinel-token-abcdef123456',
  });
  const tree = {
    headline: 'ok',
    count: 42,
    flags: ['keep', { note: `token: ${envLeak.GITHUB_TOKEN}` }],
    evidence: {
      stdout: `api_key: "${SENTINEL}"`,
      clean: 'nothing to see',
    },
  };
  const redacted = redactRecord(tree, { env: envLeak, apiKeyEnv: 'ROSTER_API_KEY' });
  const text = textTree(redacted);
  assert.ok(!text.includes(SENTINEL), 'nested api_key value must be redacted');
  assert.ok(!text.includes(envLeak.GITHUB_TOKEN), 'nested env token must be redacted');
  assert.equal(redacted.headline, 'ok', 'non-secret strings survive');
  assert.equal(redacted.count, 42, 'non-string values survive');
  assert.deepEqual(redacted.flags[0], 'keep', 'non-secret list items survive');
  assert.equal(redacted.evidence.clean, 'nothing to see', 'non-secret nested strings survive');
});

test('buildProvenanceRecord: secrets in payload never reach the built record (requested vs served model preserved)', () => {
  // The sentinel is delivered through a realistic secret context: an api_key
  // assignment, and as the value of an env-secret-bearing assignment. (The
  // typed API redacts with its own env options, so env-value redaction is
  // covered directly by redactEvidence/redactRecord above.)
  const record = buildProvenanceRecord(
    {
      runId: 'run-2025-0101-001',
      sessionId: 'sess-2025-0101-001',
      event: 'completed',
      payload: {
        requestedModel: 'gpt-large-preview',
        servedModel: 'gpt-large-stable',
        modelDiscrepancy: true,
        evidence: {
          headers: `api_key: "${SENTINEL}"`,
          notes: 'requested model differed from served model',
        },
      },
    },
    { repoIdentity: REPO_IDENTITY, now: NOW },
  );

  const text = textTree(record);
  assertNoSecretsPersisted(text, 'built provenance record');

  // The requested-vs-served model discrepancy is preserved, not redacted.
  assert.equal(record.payload.requestedModel, 'gpt-large-preview');
  assert.equal(record.payload.servedModel, 'gpt-large-stable');
  assert.equal(record.payload.modelDiscrepancy, true);
  assert.equal(record.createdAt, NOW, 'injected clock stamps createdAt deterministically');
  assert.equal(record.repoIdentity, REPO_IDENTITY, 'stable repo identity is carried through');
});

test('createRecord: unknown usage metrics are marked unknown, known metrics stay numeric, discrepancy survives', () => {
  const record = createRecord(
    {
      run: { id: 'run-2025-0101-002' },
      session: { id: 'sess-2025-0101-002' },
      requestedModel: 'gpt-large-preview',
      servedModel: 'gpt-large-stable',
      outcome: 'succeeded',
      metrics: {
        duration_ms: 1200,
        cost_usd: 'pending', // invalid known metric -> unknown
        // unknown metric field -> preserved but flagged unknown
        tokens_refund: 7,
        tokens_prompt: 'N/A', // invalid known metric -> unknown
      },
    },
    NOW,
  );

  assert.equal(record.metrics.duration_ms, 1200, 'valid known metric keeps numeric value');
  assert.equal(record.metrics.cost_usd, 'unknown', 'invalid known metric marked unknown');
  assert.equal(record.metrics.tokens_prompt, 'unknown', 'invalid known metric marked unknown');
  assert.equal(record.metrics.tokens_completion, 'unknown', 'missing known metric marked unknown');
  assert.equal(record.metrics.tool_calls, 'unknown', 'missing known metric marked unknown');
  assert.equal(record.metrics.retry_count, 'unknown', 'missing known metric marked unknown');
  assert.equal(record.metrics.tokens_refund, 'unknown', 'unknown metric name preserved but flagged');
  assert.equal(record.requestedModel, 'gpt-large-preview', 'requested model preserved');
  assert.equal(record.servedModel, 'gpt-large-stable', 'served model preserved');
  assert.notEqual(record.requestedModel, record.servedModel, 'discrepancy is observable');
  assert.equal(record.createdAt, NOW, 'injected clock stamps createdAt');
});

test('ProvenanceStore.record: secrets in event payloads are redacted in the persisted record (failure and cancellation events)', async (t) => {
  const dir = await t.testContext?.tmpDir?.() ?? null;
  // Use an OS temp dir to keep this test hermetic and cleanup-tracked.
  const { mkdtemp, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const root = await (dir ? Promise.resolve(dir) : mkdtemp(join(tmpdir(), 'prov-redact-')));
  try {
    const store = createProvenanceStore({ root, repoRoot: process.cwd() });
    const identity = await store.identity();

    // Failure event with secret-rich payload (realistic: a failed run dumps
    // its request config into the evidence).
    const failureRecord = await store.recordEvent({
      runId: 'run-2025-0101-003',
      sessionId: 'sess-2025-0101-003',
      event: 'failure',
      payload: {
        requestedModel: 'gpt-large-preview',
        servedModel: 'gpt-large-stable',
        modelDiscrepancy: true,
        error: '500 upstream',
        evidence: {
          config: `api_key: "${SENTINEL}"`,
          key: `-----BEGIN RSA PRIVATE KEY-----\n${'PLACEHOLDERNOTAKEY'.repeat(4)}\n-----END RSA PRIVATE KEY-----`,
          metrics: {
            duration_ms: 900,
            tokens_prompt: 'unknown-usage', // invalid -> unknown
            speculative_cache: 3, // unknown metric -> flagged
          },
        },
      },
    });

    // Cancellation event.
    const cancelRecord = await store.recordEvent({
      runId: 'run-2025-0101-003',
      sessionId: 'sess-2025-0101-003',
      event: 'cancellation',
      payload: {
        requestedModel: 'gpt-large-preview',
        servedModel: 'gpt-large-stable',
        // Realistic secret context: the api_key assignment is what the
        // redaction stack must strip; a bare unquoted token is not a
        // credential-assignment form.
        evidence: { config: `api_key: "${SENTINEL}"`, note: 'user aborted run mid-flight' },
      },
    });

    // The in-memory returned records are redacted.
    for (const [label, record] of [
      ['failure record', failureRecord],
      ['cancellation record', cancelRecord],
    ]) {
      const text = textTree(record);
      assertNoSecretsPersisted(text, label);
    }
    assert.equal(failureRecord.payload.requestedModel, 'gpt-large-preview');
    assert.equal(failureRecord.payload.servedModel, 'gpt-large-stable');

    // Unknown usage metrics survive redaction intact (they are not secrets).
    const failureMetrics = failureRecord.payload.evidence.metrics;
    assert.equal(failureMetrics.duration_ms, 900);
    assert.equal(failureMetrics.tokens_prompt, 'unknown-usage', 'unknown usage preserved verbatim');
    assert.equal(failureMetrics.speculative_cache, 3);

    // The records read back from the durable store are redacted too: this is
    // the persisted surface the check actually cares about.
    const queried = await store.query({ runId: 'run-2025-0101-003' });
    assert.ok(queried.length >= 2, 'both lifecycle events are queryable');
    for (const record of queried) {
      const text = textTree(record);
      assertNoSecretsPersisted(text, `persisted record ${record.event}`);
    }
    const persistedFailure = queried.find((r) => r.event === 'failure');
    assert.equal(persistedFailure.payload.requestedModel, 'gpt-large-preview');
    assert.equal(persistedFailure.payload.servedModel, 'gpt-large-stable');

    // Raw bytes on disk, not just the read API, hold neither the sentinel nor the key body.
    const { readdir, readFile } = await import('node:fs/promises');
    const files = (await readdir(root, { recursive: true, withFileTypes: true })).filter((entry) => entry.isFile());
    assert.ok(files.length >= 2, 'records were persisted to disk');
    for (const entry of files) {
      const raw = await readFile(join(entry.parentPath ?? entry.path, entry.name), 'utf8');
      assertNoSecretsPersisted(raw, `raw file ${entry.name}`);
      assert.ok(!raw.includes('PLACEHOLDERNOTAKEY'), `raw file ${entry.name} must not hold the key body`);
    }

    void identity;
  } finally {
    if (!dir) await rm(root, { recursive: true, force: true });
  }
});
