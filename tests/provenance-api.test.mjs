import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, mkdir, writeFile, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

import {
  ProvenanceStore,
  createProvenanceStore,
  instrumentLifecycle,
  createRoutingReader,
  validateProvenanceRecord,
  compareIdentity,
  openProvenanceStore,
  buildProvenanceRecord,
} from '../src/lib/provenance-api.mjs';
import { identityHash, resolveRepoIdentity } from '../src/lib/repo-identity.mjs';
import {
  captureLifecycleEvent,
  lifecycleHooks,
  provenanceOptOut,
} from '../src/lib/local-runs.mjs';

// --- helpers ---------------------------------------------------------------

// Build a git-runner fake for resolveRepoIdentity so two distinct repositories
// get distinct identities without touching the real filesystem.
function fakeGit({ commonDir, remoteUrl }) {
  return async (program, args) => {
    if (args.includes('rev-parse') && args.includes('--git-common-dir')) {
      return { stdout: `${commonDir}\n` };
    }
    if (args.includes('config') && args.includes('remote.origin.url')) {
      return { stdout: `${remoteUrl}\n` };
    }
    throw new Error(`unexpected git args: ${args.join(' ')}`);
  };
}

async function makeStore({ root, commonDir, remoteUrl, repoRoot = '/repo' }) {
  const dir = await mkdtemp(path.join(root, 'prov-'));
  const git = fakeGit({ commonDir, remoteUrl });
  const store = new ProvenanceStore({
    root: dir,
    resolveIdentity: () => resolveRepoIdentity({ repoRoot, run: git }),
  });
  return { dir, store };
}

const REPO_A = { commonDir: '/repo-a/.git', remoteUrl: 'https://github.com/acme/widget.git' };
const REPO_B = { commonDir: '/repo-b/.git', remoteUrl: 'https://github.com/acme/gadget.git' };

async function identityOf(commonDir, remoteUrl, repoRoot = '/repo') {
  return resolveRepoIdentity({ repoRoot, run: fakeGit({ commonDir, remoteUrl }) });
}

// --- identity: non-collision across repos ----------------------------------

test('two distinct repositories produce non-colliding identities', async () => {
  const a = await identityOf(REPO_A.commonDir, REPO_A.remoteUrl);
  const b = await identityOf(REPO_B.commonDir, REPO_B.remoteUrl);
  assert.notEqual(a, b);
  // Identity is a hash form that cannot be a directory name and never leaks
  // the raw remote URL or git dir.
  assert.match(a, /^[a-z0-9]+-[a-f0-9]{64}$/);
  assert.doesNotMatch(a, /widget|gadget|repo-a|repo-b|\//);
});

test('the same repository resolves to the same identity regardless of run', async () => {
  const a1 = await identityOf(REPO_A.commonDir, REPO_A.remoteUrl);
  const a2 = await identityOf(REPO_A.commonDir, REPO_A.remoteUrl);
  assert.equal(a1, a2);
});

test('compareIdentity reports mismatch for a different repository', () => {
  assert.equal(compareIdentity(undefined, 'sha256-abc').status, 'initialize');
  assert.equal(compareIdentity('sha256-abc', 'sha256-abc').status, 'match');
  assert.equal(compareIdentity('sha256-abc', 'sha256-def').status, 'mismatch');
});

// --- typed validation ------------------------------------------------------

test('validateProvenanceRecord enforces the five lifecycle events', () => {
  const ok = { repoIdentity: 'sha256-' + 'a'.repeat(64), runId: 'r1', sessionId: 's1', event: 'started' };
  assert.deepEqual(validateProvenanceRecord(ok), { ...ok, payload: {} });
  for (const event of ['session', 'failure', 'cancellation', 'completed']) {
    validateProvenanceRecord({ ...ok, event });
  }
  assert.throws(() => validateProvenanceRecord({ ...ok, event: 'bogus' }), TypeError);
  assert.throws(() => validateProvenanceRecord({ ...ok, runId: '' }), TypeError);
  assert.throws(() => validateProvenanceRecord({ ...ok, repoIdentity: 'no-hash' }), TypeError);
});

test('typed validation rejects incompatible or malformed schema versions', () => {
  const record = { runId: 'schema-run', sessionId: 'schema-session', event: 'completed' };
  for (const schemaVersion of ['99.0.0', '0.1.0', 'invalid', '1.0.0-01', null]) {
    assert.throws(() => validateProvenanceRecord({ ...record, schemaVersion }), /schemaVersion/);
  }
  for (const schemaVersion of ['1.9.0', '1.0.1', '1.2.0-rc.1+fixture']) {
    assert.doesNotThrow(() => validateProvenanceRecord({ ...record, schemaVersion }));
  }
});

test('typed persistence refuses nested prompt/source snapshots before writing', async (t) => {
  const root = await mkdtemp(path.join(tmpdir(), 'prov-snapshots-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const { dir, store } = await makeStore({ root, ...REPO_A });
  for (const key of ['prompt', 'source', 'promptBody', 'source_body', 'rawPrompt',
    'system_prompt', 'sourceCode', 'file_contents', 'messages', 'transcript']) {
    const payload = { evidence: [{ [key]: 'PROTECTED_BODY_FIXTURE_195' }] };
    assert.throws(() => buildProvenanceRecord({
      runId: 'snapshot-run', sessionId: 'snapshot-session', event: 'failure', payload,
    }), /snapshot|body/i);
    await assert.rejects(() => store.recordEvent({
      runId: 'snapshot-run', sessionId: 'snapshot-session', event: 'failure', payload,
    }), /snapshot|body/i);
    await assert.rejects(() => store.recordMemory({
      runId: 'snapshot-run', sessionId: 'snapshot-session', memory: 'curated note', payload,
    }), /snapshot|body/i);
  }
  assert.deepEqual(await store.underlying.readAll(), { records: [], skipped: [] });
  assert.deepEqual(await readdir(dir), []);
});

test('typed payload validation rejects malformed data and preserves diagnostic metadata and usage', async (t) => {
  const root = await mkdtemp(path.join(tmpdir(), 'prov-metadata-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const { store } = await makeStore({ root, ...REPO_A });
  const circular = {};
  circular.self = circular;
  for (const payload of [null, 'body', [], { duration: NaN }, { output: undefined },
    { values: new Array(1) }, circular, { timestamp: new Date(0) }]) {
    await assert.rejects(() => store.recordEvent({
      runId: 'metadata-run', sessionId: 'metadata-session', event: 'completed', payload,
    }), TypeError);
  }
  const payload = {
    summary: 'Tool failed; source is referenced, not captured',
    sourceRef: 'fixture.mjs:12', promptRef: 'prompt-195',
    requestedModel: 'requested-fixture', servedModel: 'served-fixture',
    metrics: { tokens_prompt: 123, tokens_completion: 'unknown', cost_usd: null },
    evidence: { output: 'api_key = "test-only-private-api-key"', exitCode: 1 },
  };
  await store.recordEvent({
    runId: 'metadata-run', sessionId: 'metadata-session', event: 'completed', payload,
  });
  const [persisted] = await store.query({});
  assert.deepEqual(persisted.payload.metrics, payload.metrics);
  assert.equal(persisted.payload.sourceRef, payload.sourceRef);
  assert.equal(persisted.payload.promptRef, payload.promptRef);
  assert.equal(persisted.payload.summary, payload.summary);
  assert.equal(persisted.payload.requestedModel, payload.requestedModel);
  assert.equal(persisted.payload.servedModel, payload.servedModel);
  assert.match(persisted.payload.evidence.output, /REDACTED/);
});

test('typed queries surface incompatible persisted records instead of silently omitting them', async (t) => {
  const root = await mkdtemp(path.join(tmpdir(), 'prov-query-schema-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const { dir, store } = await makeStore({ root, ...REPO_A });
  await mkdir(path.join(dir, 'log'), { recursive: true });
  await writeFile(path.join(dir, 'log', 'incompatible.json'), JSON.stringify({
    id: 'incompatible', version: 1, schemaVersion: '99.0.0',
    repoIdentity: await store.identity(), section: 'raw-history',
    runId: 'schema-run', sessionId: 'schema-session', event: 'completed',
  }));
  assert.equal((await store.underlying.readAll()).skipped.length, 1);
  await assert.rejects(() => store.query({}), /quarantine|repair/i);
  const repaired = await store.underlying.repair();
  assert.equal(repaired.quarantined.length, 1);
  assert.deepEqual(await store.query({}), []);
});

// --- lifecycle: five required cases ----------------------------------------

test('typed records project supplied canonical evidence without fabricating metrics or mutating payloads', async (t) => {
  const root = await mkdtemp(path.join(tmpdir(), 'prov-projection-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const { store } = await makeStore({ root, ...REPO_A });
  const payload = {
    repository: { commit: 'fixture-commit' },
    issue: { issue: '42', task: 'issue-42' },
    seat: { name: 'coder' }, route: { name: 'fixture-route' },
    requestedModel: 'requested-fixture', servedModel: 'served-fixture',
    startedAt: 1000, endedAt: 2000, outcome: 'failed',
    tools: { name: 'fixture-tool', version: '1' },
    metrics: { tokens_prompt: 123, tokens_completion: 0, cost_usd: null },
    evidence: { check: 'fixture-check', verdict: 'fail' },
  };
  const original = JSON.stringify(payload);
  const built = buildProvenanceRecord({
    runId: 'projection-run', sessionId: 'projection-session', event: 'failure', payload,
  });
  assert.equal(built.requestedModel, payload.requestedModel);
  assert.equal(built.servedModel, payload.servedModel);
  assert.equal(built.repository.commit, 'fixture-commit');
  assert.deepEqual(built.issue, payload.issue);
  assert.deepEqual(built.seat, payload.seat);
  assert.deepEqual(built.route, payload.route);
  assert.equal(built.startedAt, 1000);
  assert.equal(built.endedAt, 2000);
  assert.equal(built.outcome, 'failed');
  assert.deepEqual(built.tools, payload.tools);
  assert.deepEqual(built.evidence, payload.evidence);
  assert.equal(built.metrics.tokens_prompt, 123);
  assert.equal(built.metrics.tokens_completion, 0);
  assert.equal(built.metrics.cost_usd, 'unknown');
  assert.equal(built.metrics.duration_ms, 'unknown');
  await store.recordEvent({
    runId: 'projection-run', sessionId: 'projection-session', event: 'failure', payload,
  });
  const [persisted] = await store.query({});
  assert.equal(persisted.servedModel, 'served-fixture');
  assert.equal(persisted.metrics.tokens_completion, 0);
  assert.deepEqual(persisted.payload, payload);
  assert.equal(JSON.stringify(payload), original);
});

test('the typed API records and queries all five lifecycle events', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'lifecycle-'));
  try {
    const { store } = await makeStore({ root, ...REPO_A });
    const ctx = { runId: 'run-1', sessionId: 'session-1' };
    await store.recordEvent({ ...ctx, event: 'started' });
    await store.recordEvent({ ...ctx, event: 'session' });
    await store.recordEvent({ ...ctx, event: 'failure' });
    await store.recordEvent({ ...ctx, event: 'cancellation' });
    await store.recordEvent({ ...ctx, event: 'completed' });

    const all = await store.query({});
    assert.equal(all.length, 5);
    const events = all.map((record) => record.event);
    assert.deepEqual(events, ['started', 'session', 'failure', 'cancellation', 'completed']);
    for (const record of all) {
      assert.equal(record.repoIdentity, await store.identity());
      assert.equal(record.runId, 'run-1');
      assert.equal(record.sessionId, 'session-1');
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('instrumentLifecycle captures every lifecycle event through the typed API', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'instrument-'));
  try {
    const { store } = await makeStore({ root, ...REPO_A });
    const hooks = instrumentLifecycle(store);
    const ctx = { runId: 'run-2', sessionId: 'session-2' };
    await hooks.started(ctx);
    await hooks.session(ctx);
    await hooks.failure(ctx);
    await hooks.cancellation(ctx);
    await hooks.completed(ctx);

    const all = await store.query({});
    assert.equal(all.length, 5);
    assert.deepEqual(all.map((record) => record.event),
      ['started', 'session', 'failure', 'cancellation', 'completed']);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

// --- raw history vs curated memory -----------------------------------------

test('raw history stays separate from curated memory', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'separation-'));
  try {
    const { store } = await makeStore({ root, ...REPO_A });
    const ctx = { runId: 'run-3', sessionId: 'session-3' };
    await store.recordEvent({ ...ctx, event: 'started' });
    await store.recordEvent({ ...ctx, event: 'completed' });
    await store.recordMemory({ ...ctx, memory: 'preference: prefer table-driven tests' });

    const raw = await store.query({ section: 'raw-history' });
    assert.equal(raw.length, 2);
    assert.ok(raw.every((record) => typeof record.memory === 'undefined'));
    assert.ok(raw.every((record) => record.section === 'raw-history'));

    const memory = await store.query({ section: 'curated-memory' });
    assert.equal(memory.length, 1);
    assert.equal(memory[0].memory, 'preference: prefer table-driven tests');
    assert.ok(memory.every((record) => record.section === 'curated-memory'));

    // Raw history query must not return curated records and vice versa.
    const allRaw = await store.query({});
    assert.equal(allRaw.length, 2);
    assert.ok(allRaw.every((r) => r.section === 'raw-history'));
    // All raw records are valid lifecycle events only.
    const lifecycleSet = new Set(['started', 'session', 'failure', 'cancellation', 'completed']);
    assert.ok(allRaw.every((r) => lifecycleSet.has(r.event)));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('redaction strips env-sourced secrets from payload and memory before persistence', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'redact-'));
  try {
    const { store } = await makeStore({ root, ...REPO_A });
    const ctx = { runId: 'run-red', sessionId: 'session-red' };
    // A non-credential sentinel fed through a realistic secret context:
    // an api_key assignment that redactEvidence recognizes by pattern.
    const sentinel = 'test-only-private-api-key';
    await store.recordEvent({
      ...ctx,
      event: 'completed',
      payload: { output: `api_key = "${sentinel}"` },
    });
    await store.recordMemory({
      ...ctx,
      memory: `deploy uses api_key = "${sentinel}"`,
    });

    // Query back through the typed API and verify the sentinel is redacted.
    const raw = await store.query({});
    assert.equal(raw.length, 1);
    const rawJson = JSON.stringify(raw[0]);
    assert.doesNotMatch(rawJson, new RegExp(sentinel));
    assert.match(rawJson, /REDACTED/i);

    const memory = await store.query({ section: 'curated-memory' });
    assert.equal(memory.length, 1);
    assert.doesNotMatch(memory[0].memory, new RegExp(sentinel));
    assert.match(memory[0].memory, /REDACTED/i);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

// --- two repositories, no cross-repo leakage -------------------------------

test('two repositories are independently queryable with no cross-repo leakage', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'dual-'));
  try {
    const dir = await mkdtemp(path.join(root, 'store-'));
    const gitA = fakeGit(REPO_A);
    const gitB = fakeGit(REPO_B);
    const storeA = new ProvenanceStore({
      root: dir,
      resolveIdentity: () => resolveRepoIdentity({ repoRoot: '/repo-a', run: gitA }),
    });
    const storeB = new ProvenanceStore({
      root: dir,
      resolveIdentity: () => resolveRepoIdentity({ repoRoot: '/repo-b', run: gitB }),
    });

    await storeA.recordEvent({ runId: 'run-a', sessionId: 'session-a', event: 'started' });
    await storeA.recordMemory({ runId: 'run-a', sessionId: 'session-a', memory: 'A memory' });
    await storeB.recordEvent({ runId: 'run-b', sessionId: 'session-b', event: 'started' });
    await storeB.recordMemory({ runId: 'run-b', sessionId: 'session-b', memory: 'B memory' });

    // Each repo sees only its own records.
    const aRaw = await storeA.query({});
    const bRaw = await storeB.query({});
    assert.equal(aRaw.length, 1);
    assert.equal(bRaw.length, 1);
    assert.equal(aRaw[0].runId, 'run-a');
    assert.equal(aRaw[0].repoIdentity, await storeA.identity());
    assert.equal(bRaw[0].runId, 'run-b');
    assert.equal(bRaw[0].repoIdentity, await storeB.identity());

    // Querying with the other repo's identity returns nothing (no leakage).
    const aIdentity = await storeA.identity();
    const bIdentity = await storeB.identity();
    assert.notEqual(aIdentity, bIdentity);
    const leakedFromA = await storeA.query({ repoIdentity: bIdentity });
    assert.deepEqual(leakedFromA, []);
    const leakedFromB = await storeB.query({ repoIdentity: aIdentity });
    assert.deepEqual(leakedFromB, []);

    // Curated memory is also isolated per repo.
    const aMem = await storeA.query({ section: 'curated-memory' });
    const bMem = await storeB.query({ section: 'curated-memory' });
    assert.equal(aMem.length, 1);
    assert.equal(bMem.length, 1);
    assert.equal(aMem[0].memory, 'A memory');
    assert.equal(bMem[0].memory, 'B memory');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

// --- opt-out ----------------------------------------------------------------

test('opt-out suppresses durable records while the lifecycle still completes', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'optout-'));
  try {
    const { store } = await makeStore({ root, ...REPO_A });
    const hooks = instrumentLifecycle(store, { optOut: true });
    const ctx = { runId: 'run-opt', sessionId: 'session-opt' };
    const events = [];
    for (const name of ['started', 'session', 'failure', 'cancellation', 'completed']) {
      const result = await hooks[name](ctx);
      events.push(result);
      assert.equal(result.durable, false);
    }
    // All five lifecycle events fired in order, but nothing was persisted.
    assert.deepEqual(events.map((event) => event.event),
      ['started', 'session', 'failure', 'cancellation', 'completed']);
    assert.equal((await store.query({})).length, 0);
    // No records at all in the durable store.
    assert.equal((await store.query({ section: 'curated-memory' })).length, 0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

// --- routing/eval reader only through typed API ----------------------------

test('routing/eval readers consume records only through the typed API', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'reader-'));
  try {
    const { store } = await makeStore({ root, ...REPO_A });
    const ctx = { runId: 'run-r', sessionId: 'session-r' };
    await store.recordEvent({ ...ctx, event: 'started' });
    await store.recordEvent({ ...ctx, event: 'completed' });
    await store.recordMemory({ ...ctx, memory: 'curated note' });

    const reader = createRoutingReader(store);
    const runRecords = await reader.run('run-r');
    assert.equal(runRecords.length, 2);
    const completed = await reader.byEvent('completed');
    assert.equal(completed.length, 1);
    assert.equal(completed[0].event, 'completed');
    const sessions = await reader.session('session-r');
    assert.equal(sessions.length, 2);
    const memory = await reader.curatedMemory();
    assert.equal(memory.length, 1);
    assert.equal(memory[0].memory, 'curated note');
    const raw = await reader.rawHistory();
    assert.equal(raw.length, 2);

    // The reader object exposes no filesystem-level surface.
    for (const method of ['readdir', 'readFile', 'stat', 'traverse', 'fs']) {
      assert.equal(method in reader, false);
    }
    assert.equal(typeof reader.root, 'undefined');

    // The store does not expose raw filesystem traversal.
    assert.equal(typeof store.readdir, 'undefined');
    assert.equal(typeof store.readFile, 'undefined');
    assert.equal(typeof store.traverse, 'undefined');

    // The typed API is the only query path.
    const identity = await store.identity();
    assert.equal((await store.query({})).length, 2);
    assert.ok(identity);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

// --- earlier-wave store integration ----------------------------------------

test('durable I/O flows through openProvenanceStore (the earlier-wave store)', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'earlier-'));
  try {
    const dir = await mkdtemp(path.join(root, 'store-'));
    const git = fakeGit(REPO_A);
    // Explicitly open the earlier-wave store and pass it as underlying.
    const earlierStore = openProvenanceStore(dir);
    const store = new ProvenanceStore({
      root: dir,
      resolveIdentity: () => resolveRepoIdentity({ repoRoot: '/repo-a', run: git }),
      underlying: earlierStore,
    });

    await store.recordEvent({ runId: 'run-e', sessionId: 'session-e', event: 'started' });
    await store.recordMemory({ runId: 'run-e', sessionId: 'session-e', memory: 'earlier wave' });

    // The earlier-wave store's readAll sees the records we wrote.
    const { records } = await earlierStore.readAll();
    assert.equal(records.length, 2);
    // Every record carries version 1 and a safe id (required by the store).
    assert.ok(records.every((r) => r.version === 1));
    assert.ok(records.every((r) => typeof r.id === 'string' && r.id.length > 0 && /^[A-Za-z0-9._-]+$/.test(r.id)));

    // The typed API query returns the same records filtered by identity.
    const queried = await store.query({});
    assert.equal(queried.length, 1);
    const queriedMem = await store.query({ section: 'curated-memory' });
    assert.equal(queriedMem.length, 1);
    assert.equal(queriedMem[0].memory, 'earlier wave');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('createProvenanceStore opens the earlier-wave store by default', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'factory-'));
  try {
    const dir = await mkdtemp(path.join(root, 'store-'));
    const store = createProvenanceStore({
      root: dir,
      repoRoot: '/repo-a',
      run: fakeGit(REPO_A),
    });
    await store.recordEvent({ runId: 'run-f', sessionId: 'session-f', event: 'started' });
    const all = await store.query({});
    assert.equal(all.length, 1);
    assert.equal(all[0].repoIdentity, await store.identity());
    assert.notEqual(await store.identity(), await identityOf(REPO_B.commonDir, REPO_B.remoteUrl));

    // The underlying store is the earlier-wave one.
    assert.equal(typeof store.underlying.appendRecord, 'function');
    assert.equal(typeof store.underlying.readAll, 'function');
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

// --- run/session integration (src/lib/local-runs.mjs) -----------------------

test('local run lifecycle hooks capture all five events through the typed API', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'runhook-'));
  try {
    const { store } = await makeStore({ root, ...REPO_A });
    const hooks = lifecycleHooks(store);
    const ctx = { runId: 'run-h', sessionId: 'session-h' };
    for (const name of ['started', 'session', 'failure', 'cancellation', 'completed']) {
      const result = await hooks[name](ctx);
      assert.equal(result.durable, true);
    }
    const all = await store.query({});
    assert.deepEqual(all.map((record) => record.event),
      ['started', 'session', 'failure', 'cancellation', 'completed']);
    // The reader path sees the same durable records.
    assert.equal((await createRoutingReader(store).run('run-h')).length, 5);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('opt-out configuration keeps the lifecycle completing without durable history', async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'runopt-'));
  try {
    assert.equal(provenanceOptOut({ ROSTER_PROVENANCE_OPT_OUT: 'true' }), true);
    assert.equal(provenanceOptOut({ ROSTER_PROVENANCE_OPT_OUT: '0' }), false);
    assert.equal(provenanceOptOut({}), false);

    const { store } = await makeStore({ root, ...REPO_A });
    const hooks = lifecycleHooks(store, { optOut: provenanceOptOut({ ROSTER_PROVENANCE_OPT_OUT: '1' }) });
    const ctx = { runId: 'run-o', sessionId: 'session-o' };
    const results = [];
    for (const name of ['started', 'session', 'failure', 'cancellation', 'completed']) {
      results.push(await hooks[name](ctx));
    }
    assert.deepEqual(results.map((result) => result.event),
      ['started', 'session', 'failure', 'cancellation', 'completed']);
    assert.ok(results.every((result) => result.durable === false));
    assert.equal((await store.query({})).length, 0);
    assert.equal((await store.query({ section: 'curated-memory' })).length, 0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('captureLifecycleEvent degrades to non-durable instead of breaking a run', async () => {
  const missing = await captureLifecycleEvent(null, { event: 'started', runId: 'r', sessionId: 's' });
  assert.equal(missing.durable, false);
  assert.equal(missing.event, 'started');
  const root = await mkdtemp(path.join(tmpdir(), 'degrade-'));
  try {
    const { store } = await makeStore({ root, ...REPO_A });
    const broken = await captureLifecycleEvent(store, {
      event: 'bogus', runId: 'r', sessionId: 's',
    });
    assert.equal(broken.durable, false);
    assert.equal((await store.query({})).length, 0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
