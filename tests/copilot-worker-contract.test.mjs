import assert from 'node:assert/strict';
import { existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';

// All worker events/usage/checks are invented fixtures. Gate B does not qualify
// a real executable, account, remote cancellation, or physical containment.
test('offline full-agent worker contract — design gates B1–B4', async (t) => {
  const originalFetch = globalThis.fetch;
  let networkCalls = 0;
  globalThis.fetch = () => {
    networkCalls++;
    throw new Error('Offline network tripwire');
  };
  t.after(() => {
    globalThis.fetch = originalFetch;
    assert.equal(networkCalls, 0, 'no real network attempt');
  });
  const { createOfflineAttempt, offlineDigest, offlineHostReview } = await import('./helpers/copilot-worker-offline.mjs');

  function contract() {
    return {
      ask: 'Change the approved sample, preserving the task checks.',
      task: 'Issue 611; check the sample; no scope expansion.',
      base: 'reviewed-base-fixture', run: 'offline-run', attempt: 'attempt-1',
      scope: { read: ['sample.txt'], write: ['sample.txt', 'remove.txt'], delete: ['remove.txt'] },
      config: { requestedModel: 'requested-fixture-model', executable: 'not-a-real-cli', version: 'fixture-v1' },
      limits: { files: 2, bytes: 64, callbacks: 4, events: 12, duration: 100 },
    };
  }
  async function fixture(context, options = {}) {
    const root = mkdtempSync(path.join(process.cwd(), '.copilot-worker-offline-'));
    let attempt;
    context.after(() => {
      try { attempt?.dispose(); }
      finally { rmSync(root, { recursive: true, force: true }); }
    });
    writeFileSync(path.join(root, 'sample.txt'), 'before\n');
    writeFileSync(path.join(root, 'remove.txt'), 'remove\n');
    writeFileSync(path.join(root, 'TASK.md'), 'immutable task\n');
    attempt = await createOfflineAttempt({ root, contract: contract(), ...options });
    return { root, attempt };
  }
  function inference(attempt, id = 'call-1', extra = {}) {
    attempt.observe({ type: 'inference', id, ...extra });
  }
  function complete(attempt) {
    attempt.observe({ type: 'idle' });
    attempt.observe({ type: 'exit', code: 0 });
    attempt.observe({ type: 'cleanup', ok: true });
  }
  function envelope(attempt, files = [{ path: 'sample.txt', content: 'after\n' }]) {
    return { kind: 'external-full-agent', evidence: 'synthetic', anchors: attempt.anchors, files };
  }
  async function candidate(context, options = {}) {
    const result = await fixture(context, options);
    inference(result.attempt);
    complete(result.attempt);
    result.attempt.finish(envelope(result.attempt));
    return result;
  }

  await t.test('B1: immutable Ask/task/base/scope/config anchors and capability descriptors', async (context) => {
    const original = contract();
    const { attempt } = await fixture(context, { contract: original });
    const before = offlineDigest(attempt.anchors);
    original.ask = 'mutated';
    original.task = 'mutated';
    original.base = 'mutated';
    original.scope.write.push('escape.txt');
    original.config.requestedModel = 'mutated';
    original.limits.bytes = 1_000_000;
    assert.equal(offlineDigest(attempt.anchors), before);
    for (const key of ['ask', 'task', 'baseDigest', 'scope', 'config']) assert.match(attempt.anchors[key], /^[a-f0-9]{64}$/);
    assert.equal(attempt.anchors.base, 'reviewed-base-fixture');
    assert.throws(() => { attempt.anchors.base = 'changed'; }, TypeError);
    assert.throws(() => { attempt.capabilities.environment.KEY = 'fake'; }, TypeError);
    assert.deepEqual(attempt.capabilities.environment, {});
    for (const key of ['network', 'launch', 'auth', 'shell', 'git', 'publisher', 'physicalContainment']) {
      assert.equal(attempt.capabilities[key], false);
    }
    assert.deepEqual(attempt.capabilities.callbacks, ['read_file']);
    const result = await attempt.callback('read_file', { path: 'sample.txt' });
    assert.match(JSON.stringify(result), /before/);
  });

  await t.test('B1/B3/B4: multi-call full-agent attempt, bounded import/deletion, independent review', async (context) => {
    const { root, attempt } = await fixture(context);
    inference(attempt, 'call-1', { servedModel: 'served-fixture-model', usage: { unit: 'tokens', value: 3, complete: true } });
    await attempt.callback('read_file', { path: 'sample.txt' });
    inference(attempt, 'call-2', { servedModel: 'served-fixture-model', usage: { unit: 'tokens', value: 5, complete: true } });
    await attempt.callback('read_file', { path: 'sample.txt' });
    complete(attempt);
    const raw = envelope(attempt, [{ path: 'sample.txt', content: 'after\n' }, { path: 'remove.txt', delete: true }]);
    const accepted = attempt.finish(raw);
    raw.files[0].content = 'caller mutation';
    assert.equal(accepted.files[0].content, 'after\n');
    const subject = attempt.importCandidate(attempt.anchors);
    assert.equal(readFileSync(path.join(root, 'sample.txt'), 'utf8'), 'after\n');
    assert.equal(existsSync(path.join(root, 'remove.txt')), false);
    assert.equal(readFileSync(path.join(root, 'TASK.md'), 'utf8'), 'immutable task\n');
    const receipt = offlineHostReview(subject, [{ source: 'synthetic-host-check', pass: true }]);
    assert.deepEqual(attempt.reviewEligibility(receipt, attempt.anchors),
      { hostReviewEligible: true, publishable: false, builtinSeat: false, aiRun: null });
    const evidence = attempt.provenance();
    assert.equal(evidence.observedCalls, 2);
    assert.equal(evidence.total, 8);
    assert.equal(evidence.mismatch, true);
    assert.equal(evidence.requestedModel, 'requested-fixture-model');
    assert.deepEqual(evidence.servedModels, ['served-fixture-model']);
    assert.equal(evidence.publishable, false);
    assert.equal(evidence.builtinSeat, false);
    assert.equal(evidence.aiRun, null);
    assert.equal(attempt.status().writerDeathProven, false);
  });

  const deniedPaths = ['../escape.txt', '/absolute.txt', 'C:/escape.txt', 'C:escape.txt',
    '\\\\server\\share\\file', 'sample.txt:stream', 'a/../sample.txt', 'a//sample.txt',
    'a/./sample.txt', 'sample.txt.', 'a/NUL.txt', 'TASK.md', 'nested/TASK.md',
    'agent-policy.yml', 'vendor/file.txt', '.github/workflows/check.yml', '.git/config',
    '.env', 'REVIEW.md', 'RESULT.md', 'AGENTS.md', 'outside.txt'];
  for (const relative of deniedPaths) {
    await t.test(`B1: reject artifact ${relative}`, async (context) => {
      const { root, attempt } = await fixture(context);
      inference(attempt);
      complete(attempt);
      assert.throws(() => attempt.finish(envelope(attempt, [{ path: relative, content: 'x' }])));
      assert.equal(attempt.status().state, 'blocked');
      assert.equal(readFileSync(path.join(root, 'sample.txt'), 'utf8'), 'before\n');
    });
  }

  const malformedFiles = [
    [], [{ path: 'sample.txt', content: 'x'.repeat(65) }],
    [{ path: 'sample.txt', content: 'é'.repeat(33) }],
    [{ path: 'sample.txt', content: 'a' }, { path: 'sample.txt', content: 'b' }],
    [{ path: 'sample.txt', content: Buffer.from('bytes') }],
    [{ path: 'sample.txt', content: '\ud800' }],
    [{ path: 'sample.txt', content: 'x', mode: 'symlink' }],
    [{ path: 'sample.txt', content: 'x', delete: true }],
    [{ path: 'sample.txt', delete: true }], [{ path: 'sample.txt' }],
    [{ path: 'remove.txt', delete: false }],
  ];
  for (const [index, files] of malformedFiles.entries()) {
    await t.test(`B1: malformed/duplicate/oversize/deletion artifact ${index}`, async (context) => {
      const { attempt } = await fixture(context);
      inference(attempt);
      complete(attempt);
      assert.throws(() => attempt.finish(envelope(attempt, files)));
      assert.equal(attempt.status().imported, false);
    });
  }

  for (const kind of ['directory', 'hardlink', 'junction']) {
    await t.test(`B1: reject filesystem ${kind} and post-validation replacement`, async (context) => {
      const { root, attempt } = await candidate(context);
      rmSync(path.join(root, 'sample.txt'));
      if (kind === 'directory') mkdirSync(path.join(root, 'sample.txt'));
      if (kind === 'hardlink') linkSync(path.join(root, 'remove.txt'), path.join(root, 'sample.txt'));
      if (kind === 'junction') symlinkSync(root, path.join(root, 'sample.txt'), 'junction');
      assert.throws(() => attempt.importCandidate(attempt.anchors), /Special/);
      assert.equal(attempt.status().state, 'blocked');
    });
  }

  for (const name of ['write_file', 'run_test', 'shell', 'fetch', 'auth', 'publish']) {
    await t.test(`B1: callback capability ${name} denied`, async (context) => {
      const { attempt } = await fixture(context);
      await assert.rejects(attempt.callback(name, { path: 'sample.txt' }), /capability/);
      assert.equal(attempt.status().active, 0);
    });
  }

  await t.test('B1: callback arguments snapshotted before await; exact read scope and call bound', async (context) => {
    const { attempt } = await fixture(context);
    const args = { path: 'sample.txt' };
    const pending = attempt.callback('read_file', args);
    args.path = 'TASK.md';
    assert.match(JSON.stringify(await pending), /before/);
    for (let count = 0; count < 3; count++) await attempt.callback('read_file', { path: 'sample.txt' });
    await assert.rejects(attempt.callback('read_file', { path: 'sample.txt' }), /bounded/);
    const { attempt: other } = await fixture(context);
    await assert.rejects(other.callback('read_file', { path: 'remove.txt' }), /capability/);
  });

  await t.test('B1: finite limits, protected scope, duplicate scope and accessor rejected at admission', async (context) => {
    for (const value of [0, -1, Infinity, NaN, 1.5, 1_000_001]) {
      const input = contract();
      input.limits.bytes = value;
      await assert.rejects(fixture(context, { contract: input }));
    }
    for (const files of [['TASK.md'], ['sample.txt', 'SAMPLE.TXT'], ['**/*']]) {
      const input = contract();
      input.scope.write = files;
      await assert.rejects(fixture(context, { contract: input }));
    }
    const input = contract();
    Object.defineProperty(input, 'ask', { get() { throw new Error('must not invoke getter'); }, enumerable: true });
    await assert.rejects(fixture(context, { contract: input }), /Invalid JSON property/);
  });

  await t.test('B1: cyclic, deep and oversized envelopes fail boundedly without mutation', async (context) => {
    for (const fault of ['cycle', 'depth', 'nodes', 'bytes']) {
      const { root, attempt } = await fixture(context);
      inference(attempt);
      complete(attempt);
      const raw = envelope(attempt);
      if (fault === 'cycle') raw.selfReview = raw;
      if (fault === 'depth') {
        let nested = {};
        raw.selfReview = nested;
        for (let index = 0; index < 20; index++) nested = nested.child = {};
      }
      if (fault === 'nodes') raw.selfReview = Array.from({ length: 5000 }, () => 0);
      if (fault === 'bytes') raw.files[0].content = 'x'.repeat(2_000_001);
      assert.throws(() => attempt.finish(raw), /bounds|Cyclic/);
      assert.equal(readFileSync(path.join(root, 'sample.txt'), 'utf8'), 'before\n');
    }
    const cyclic = {};
    cyclic.self = cyclic;
    assert.throws(() => offlineDigest(cyclic), /Cyclic contract JSON/);
    let deep = {};
    const input = deep;
    for (let index = 0; index < 20; index++) deep = deep.child = {};
    assert.throws(() => offlineDigest(input), /structural bounds/);
  });

  await t.test('B1: aggregate bytes, file/event bounds, malformed and builtin envelopes rejected', async (context) => {
    for (const fault of ['aggregate-bytes', 'file-count', 'envelope-shape', 'builtin', 'wrong-evidence', 'sparse-array']) {
      const input = contract();
      if (fault === 'file-count') input.limits.files = 1;
      const { attempt } = await fixture(context, { contract: input });
      inference(attempt);
      complete(attempt);
      const raw = envelope(attempt);
      if (fault === 'aggregate-bytes') raw.files = [
        { path: 'sample.txt', content: 'a'.repeat(40) }, { path: 'remove.txt', content: 'b'.repeat(40) }];
      if (fault === 'file-count') raw.files.push({ path: 'remove.txt', content: 'b' });
      if (fault === 'envelope-shape') raw.unexpected = true;
      if (fault === 'builtin') raw.kind = 'builtin-completion';
      if (fault === 'wrong-evidence') raw.evidence = 'live';
      if (fault === 'sparse-array') raw.files = new Array(1);
      assert.throws(() => attempt.finish(raw));
      assert.equal(attempt.status().state, 'blocked');
    }
    const input = contract();
    input.limits.events = 1;
    const { attempt } = await fixture(context, { contract: input });
    inference(attempt);
    assert.throws(() => inference(attempt, 'call-2'), /excess/);
  });

  await t.test('B2: pre-cancel launches no callback', async (context) => {
    const controller = new AbortController();
    controller.abort();
    let callbacks = 0;
    await assert.rejects(fixture(context, { signal: controller.signal, read: () => { callbacks++; } }), /cancelled/);
    assert.equal(callbacks, 0);
  });

  for (const stop of ['cancelled', 'deadline', 'revoked']) {
    await t.test(`B2: ${stop} during an outstanding callback drains without accepting late result`, async (context) => {
      const controller = new AbortController();
      let clock = 0;
      let resolve;
      let invoked = 0;
      const { attempt } = await fixture(context, {
        signal: controller.signal, now: () => clock,
        read: () => { invoked++; return new Promise((done) => { resolve = done; }); },
      });
      const pending = attempt.callback('read_file', { path: 'sample.txt' });
      if (stop === 'cancelled') controller.abort();
      if (stop === 'deadline') clock = 100;
      if (stop === 'revoked') attempt.revoke();
      resolve({ content: 'late' });
      await assert.rejects(pending, new RegExp(stop));
      await assert.rejects(attempt.callback('read_file', { path: 'sample.txt' }));
      assert.throws(() => inference(attempt));
      assert.throws(() => attempt.finish(envelope(attempt)));
      assert.equal(invoked, 1);
      assert.equal(attempt.status().active, 0);
      assert.equal(attempt.status().state, 'blocked');
    });
  }

  for (const stop of ['cancelled', 'deadline', 'revoked', 'late-idle', 'late-result']) {
    await t.test(`B2: ${stop} after candidate cannot revive import/review`, async (context) => {
      const controller = new AbortController();
      let clock = 0;
      const { root, attempt } = await candidate(context, { signal: controller.signal, now: () => clock });
      if (stop === 'cancelled') controller.abort();
      if (stop === 'deadline') clock = 100;
      if (stop === 'revoked') attempt.revoke();
      if (stop === 'late-idle') assert.throws(() => attempt.observe({ type: 'idle' }), /Late/);
      if (stop === 'late-result') assert.throws(() => attempt.finish(envelope(attempt)));
      assert.throws(() => attempt.importCandidate(attempt.anchors));
      assert.throws(() => attempt.reviewEligibility({}, attempt.anchors));
      assert.equal(readFileSync(path.join(root, 'sample.txt'), 'utf8'), 'before\n');
    });
  }

  await t.test('B2: cancellation after import invalidates review; callback failure and premature idle drain', async (context) => {
    const controller = new AbortController();
    const { attempt } = await candidate(context, { signal: controller.signal });
    const subject = attempt.importCandidate(attempt.anchors);
    const receipt = offlineHostReview(subject, [{ source: 'synthetic-host-check', pass: true }]);
    controller.abort();
    assert.throws(() => attempt.reviewEligibility(receipt, attempt.anchors), /cancelled/);
    await assert.rejects(attempt.callback('read_file', { path: 'sample.txt' }), /cancelled/);
    const { attempt: failing } = await fixture(context, { read: async () => { throw new Error('fixture read failure'); } });
    await assert.rejects(failing.callback('read_file', { path: 'sample.txt' }), /fixture read failure/);
    assert.equal(failing.status().active, 0);
    let resolve;
    const { attempt: pending } = await fixture(context, { read: () => new Promise((done) => { resolve = done; }) });
    const result = pending.callback('read_file', { path: 'sample.txt' });
    assert.throws(() => pending.observe({ type: 'idle' }), /active callback/);
    resolve({ content: 'late' });
    await assert.rejects(result, /active callback/);
    assert.equal(pending.status().active, 0);
  });

  await t.test('B2: deterministic injected clock is monotonic; disposal revokes all permissions', async (context) => {
    let clock = 0;
    const { attempt } = await fixture(context, { now: () => clock });
    clock = 10;
    inference(attempt);
    clock = 5;
    assert.throws(() => inference(attempt, 'call-2'), /invalid-clock/);
    const { attempt: disposed } = await candidate(context);
    disposed.dispose();
    await assert.rejects(disposed.callback('read_file', { path: 'sample.txt' }), /disposed/);
    assert.throws(() => disposed.importCandidate(disposed.anchors), /disposed/);
    assert.throws(() => disposed.reviewEligibility({}, disposed.anchors), /disposed/);
  });

  for (const fault of ['missing-idle', 'missing-cleanup', 'exit-failure', 'cleanup-failure', 'duplicate-inference',
    'duplicate-idle', 'unknown-event', 'malformed-usage']) {
    await t.test(`B2/B3: ${fault} blocks candidate; never claims writer death`, async (context) => {
      const { attempt } = await fixture(context);
      inference(attempt);
      if (fault === 'missing-idle') assert.throws(() => attempt.observe({ type: 'exit', code: 0 }));
      if (fault === 'missing-cleanup') {
        attempt.observe({ type: 'idle' });
        attempt.observe({ type: 'exit', code: 0 });
      }
      if (fault === 'exit-failure') {
        attempt.observe({ type: 'idle' });
        assert.throws(() => attempt.observe({ type: 'exit', code: 1 }));
      }
      if (fault === 'cleanup-failure') {
        attempt.observe({ type: 'idle' });
        attempt.observe({ type: 'exit', code: 0 });
        assert.throws(() => attempt.observe({ type: 'cleanup', ok: false }), /cleanup failure/);
      }
      if (fault === 'duplicate-inference') assert.throws(() => inference(attempt), /duplicate/);
      if (fault === 'duplicate-idle') {
        attempt.observe({ type: 'idle' });
        assert.throws(() => attempt.observe({ type: 'idle' }), /Duplicate/);
      }
      if (fault === 'unknown-event') assert.throws(() => attempt.observe({ type: 'model-response' }));
      if (fault === 'malformed-usage') {
        assert.throws(() => inference(attempt, 'call-2', { usage: { unit: 'tokens', value: -1, complete: true } }));
      }
      assert.throws(() => attempt.finish(envelope(attempt)));
      assert.equal(attempt.status().state, 'blocked');
      assert.equal(attempt.status().writerDeathProven, false);
      assert.equal(attempt.status().syntheticCleanupComplete, false);
    });
  }

  const usageFixtures = [
    { name: 'unknown', events: [{}, {}], status: 'unknown', total: null, model: 'unknown' },
    { name: 'zero', events: [{ servedModel: 'fixture', usage: { unit: 'tokens', value: 0, complete: true } }],
      status: 'zero', total: 0, model: 'known' },
    { name: 'incomplete missing event', events: [{ usage: { unit: 'tokens', value: 4, complete: true } }, {}],
      status: 'incomplete', total: null, model: 'unknown' },
    { name: 'reported partial', events: [{ usage: { unit: 'tokens', value: 4, complete: false } }],
      status: 'incomplete', total: null, model: 'unknown' },
    { name: 'mixed units/models', events: [
      { servedModel: 'first', usage: { unit: 'tokens', value: 4, complete: true } },
      { servedModel: 'second', usage: { unit: 'ai-credits', value: 2, complete: true } }],
    status: 'mixed', total: null, model: 'mixed' },
    { name: 'known plus missing model', events: [{ servedModel: 'first' }, {}],
      status: 'unknown', total: null, model: 'unknown' },
    { name: 'unsafe total', events: [
      { usage: { unit: 'tokens', value: Number.MAX_SAFE_INTEGER, complete: true } },
      { usage: { unit: 'tokens', value: 1, complete: true } }],
    status: 'incomplete', total: null, model: 'unknown' },
  ];
  for (const entry of usageFixtures) {
    await t.test(`B3: ${entry.name} remains distinct synthetic provenance`, async (context) => {
      const { attempt } = await fixture(context);
      entry.events.forEach((event, index) => inference(attempt, `call-${index}`, event));
      const result = attempt.provenance();
      assert.equal(result.usageStatus, entry.status);
      assert.equal(result.total, entry.total);
      assert.equal(result.modelStatus, entry.model);
      assert.equal(result.aiRun, null);
      assert.equal(result.publishable, false);
      assert.equal(result.builtinSeat, false);
    });
  }

  for (const anchor of ['ask', 'task', 'base', 'baseDigest', 'scope', 'config', 'run', 'attempt']) {
    await t.test(`B4: ${anchor} drift rejects envelope, import and review`, async (context) => {
      const { attempt } = await fixture(context);
      inference(attempt);
      complete(attempt);
      const changed = { ...attempt.anchors, [anchor]: 'changed' };
      assert.throws(() => attempt.finish({ ...envelope(attempt), anchors: changed }), /drift/);
      const { attempt: importing } = await candidate(context);
      assert.throws(() => importing.importCandidate({ ...importing.anchors, [anchor]: 'changed' }), /drift/);
      const { attempt: reviewing } = await candidate(context);
      const subject = reviewing.importCandidate(reviewing.anchors);
      const receipt = offlineHostReview(subject, [{ source: 'synthetic-host-check', pass: true }]);
      assert.throws(() => reviewing.reviewEligibility(receipt, { ...reviewing.anchors, [anchor]: 'changed' }), /drift/);
    });
  }

  await t.test('B4: self review, copied receipt, stale receipt and byte drift cannot authorize', async (context) => {
    for (const fault of ['self-review', 'copied-receipt', 'stale-receipt', 'byte-drift']) {
      const { root, attempt } = await candidate(context);
      const subject = attempt.importCandidate(attempt.anchors);
      let receipt = offlineHostReview(subject, [{ source: 'synthetic-host-check', pass: true }]);
      if (fault === 'self-review') receipt = { source: 'worker', verdict: 'pass', subject: offlineDigest(subject) };
      if (fault === 'copied-receipt') receipt = structuredClone(receipt);
      if (fault === 'stale-receipt') receipt = offlineHostReview({ ...subject, candidate: 'old-bytes' },
        [{ source: 'synthetic-host-check', pass: true }]);
      if (fault === 'byte-drift') writeFileSync(path.join(root, 'sample.txt'), 'changed after review\n');
      assert.throws(() => attempt.reviewEligibility(receipt, attempt.anchors));
      assert.equal(attempt.status().publishable, false);
    }
    assert.throws(() => offlineHostReview({}, []), /trusted/);
    assert.throws(() => offlineHostReview({}, [{ source: 'worker', pass: true }]), /trusted/);
  });
});
