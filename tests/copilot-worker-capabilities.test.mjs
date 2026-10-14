import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { assessCopilotWorkerCapabilities } from '../src/lib/copilot-worker-capabilities.mjs';

function report() {
  return {
    schema: 1, kind: 'external-full-agent', evidence: 'synthetic', concurrency: 1,
    tools: ['read_file'],
    authority: { launch: false, network: false, auth: false, shell: false, git: false, publisher: false },
    limits: { files: 16, bytes: 1_000_000, callbacks: 32, events: 128, duration: 60_000 },
    controls: { immutableTask: true, exactScope: true, boundedCallbacks: true, candidateHashes: true,
      cancellation: true, deadline: true, exitAndCleanup: true, separateProvenance: true, independentReview: true },
  };
}

test('complete synthetic declarations qualify only the offline contract, never live dispatch or publication', () => {
  const result = assessCopilotWorkerCapabilities(report());
  assert.equal(result.status, 'offline-contract-compatible');
  assert.equal(result.offlineContractCompatible, true);
  assert.deepEqual(result.unknownControls, []);
  assert.deepEqual(result.unsupportedControls, []);
  assert.equal(result.kind, 'external-full-agent');
  assert.equal(result.evidence, 'synthetic');
  assert.equal(result.liveDispatchEligible, false);
  assert.equal(result.publicationEligible, false);
  assert.deepEqual(result.liveBlockers, [
    'production-capability-bridge-unqualified', 'physical-containment-unqualified',
    'inference-auth-unapproved', 'account-data-model-spend-unapproved',
    'remote-cancellation-unqualified', 'authoring-provenance-unverified', 'dependency-gate-unresolved',
  ]);
  assert.equal(Object.hasOwn(result, 'model'), false);
  assert.equal(Object.hasOwn(result, 'usage'), false);
});

test('every missing, unknown or explicitly unsupported control blocks offline compatibility', () => {
  for (const key of Object.keys(report().controls)) {
    for (const state of ['missing', null, false]) {
      const input = report();
      if (state === 'missing') delete input.controls[key];
      else input.controls[key] = state;
      const result = assessCopilotWorkerCapabilities(input);
      assert.equal(result.status, 'blocked', `${key}: ${state}`);
      assert.equal(result.offlineContractCompatible, false);
      assert.deepEqual(result.unknownControls, state === false ? [] : [key]);
      assert.deepEqual(result.unsupportedControls, state === false ? [key] : []);
      assert.equal(result.capabilities.controls[key], state === false ? false : null);
      assert.equal(result.liveDispatchEligible, false);
      assert.equal(result.publicationEligible, false);
    }
  }
  const result = assessCopilotWorkerCapabilities({ ...report(), controls: {} });
  assert.deepEqual(result.unknownControls, Object.keys(report().controls));
  const mixed = report();
  delete mixed.controls.immutableTask;
  mixed.controls.cancellation = false;
  mixed.controls.deadline = null;
  const assessment = assessCopilotWorkerCapabilities(mixed);
  assert.deepEqual(assessment.unknownControls, ['immutableTask', 'deadline']);
  assert.deepEqual(assessment.unsupportedControls, ['cancellation']);
});

test('reports cannot claim builtin identity, measured/live evidence, fan-out or additional authority', () => {
  for (const changes of [
    { schema: 2 }, { schema: '1' }, { kind: 'builtin' }, { evidence: 'verified' },
    { evidence: 'live' }, { concurrency: 2 }, { concurrency: 0 }, { concurrency: '1' },
    { environment: {} }, { physicalContainment: true }, { model: 'requested-only' },
    { transport: () => assert.fail('transport invoked') }, { token: 'synthetic-token' },
  ]) {
    assert.throws(() => assessCopilotWorkerCapabilities({ ...report(), ...changes }), TypeError);
  }
  for (const key of Object.keys(report().authority)) {
    for (const value of [true, null, 0, 'false', undefined]) {
      const input = report();
      input.authority[key] = value;
      assert.throws(() => assessCopilotWorkerCapabilities(input), /authority must be false/);
    }
    const input = report();
    delete input.authority[key];
    assert.throws(() => assessCopilotWorkerCapabilities(input), /documented fields/);
  }
});

test('unknown, automatic and expanded callbacks are denied, not silently filtered', () => {
  for (const tools of [
    [], ['read_file', 'read_file'], ['read_file', 'write_file'], ['shell'], ['filesystem'],
    ['network'], ['mcp'], ['github'], ['publish'], ['git'], ['READ_FILE'], [null],
    'read_file', new Array(1), Object.assign(['read_file'], { extra: true }),
  ]) {
    assert.throws(() => assessCopilotWorkerCapabilities({ ...report(), tools }), /only read_file/);
  }
});

test('each budget must be explicit and finite with no unlimited or silent default value', () => {
  for (const key of Object.keys(report().limits)) {
    for (const value of [0, -1, 1.5, Infinity, NaN, Number.MAX_SAFE_INTEGER + 1, '1', null]) {
      const input = report();
      input.limits[key] = value;
      assert.throws(() => assessCopilotWorkerCapabilities(input), /positive safe integers/);
    }
    const input = report();
    delete input.limits[key];
    assert.throws(() => assessCopilotWorkerCapabilities(input), /documented fields/);
  }
  for (const value of [1, Number.MAX_SAFE_INTEGER]) {
    const input = report();
    for (const key of Object.keys(input.limits)) input.limits[key] = value;
    assert.deepEqual(assessCopilotWorkerCapabilities(input).capabilities.limits, input.limits);
  }
});

test('closed records reject unknown fields, non-JSON values and accessors without executing them', () => {
  for (const section of ['authority', 'limits', 'controls']) {
    const input = report();
    input[section].extra = true;
    assert.throws(() => assessCopilotWorkerCapabilities(input), /documented fields/);
    for (const value of [null, [], new Date(), Object.create(null)]) {
      assert.throws(() => assessCopilotWorkerCapabilities({ ...report(), [section]: value }), /plain record/);
    }
  }
  for (const value of [null, [], new Date(), Object.create(null)]) {
    assert.throws(() => assessCopilotWorkerCapabilities(value), /plain record/);
  }
  for (const value of [undefined, 'true', 1, {}, () => true]) {
    const input = report();
    input.controls.exactScope = value;
    assert.throws(() => assessCopilotWorkerCapabilities(input), /true, false or null/);
  }
  for (const section of ['root', 'authority', 'limits', 'controls', 'tools']) {
    const input = report();
    const target = section === 'root' ? input : input[section];
    const key = Object.keys(target)[0];
    Object.defineProperty(target, key, { enumerable: true, get() { assert.fail('accessor executed'); } });
    assert.throws(() => assessCopilotWorkerCapabilities(input), /data properties|only read_file/);
  }
  for (const section of ['root', 'authority', 'limits', 'controls', 'tools']) {
    const input = report();
    const target = section === 'root' ? input : input[section];
    target[Symbol('hidden')] = true;
    assert.throws(() => assessCopilotWorkerCapabilities(input), /documented fields|only read_file/);
  }
  const input = report();
  Object.defineProperty(input, 'schema', { enumerable: false });
  assert.throws(() => assessCopilotWorkerCapabilities(input), /data properties/);
  for (const key of Object.keys(report())) {
    const input = report();
    delete input[key];
    assert.throws(() => assessCopilotWorkerCapabilities(input), /documented fields/);
  }
  const callable = Object.assign(() => assert.fail('callable executed'), report());
  Object.setPrototypeOf(callable, Object.prototype);
  assert.throws(() => assessCopilotWorkerCapabilities(callable), /plain record/);
});

test('assessment snapshots are deeply immutable and independent from caller mutation', () => {
  const input = report();
  const result = assessCopilotWorkerCapabilities(input);
  input.tools[0] = 'shell';
  input.authority.auth = true;
  input.limits.duration = Infinity;
  input.controls.exactScope = false;
  assert.deepEqual(result.capabilities.tools, ['read_file']);
  assert.equal(result.capabilities.authority.auth, false);
  assert.equal(result.capabilities.limits.duration, 60_000);
  assert.equal(result.capabilities.controls.exactScope, true);
  for (const value of [result, result.capabilities, ...Object.values(result.capabilities),
    result.unknownControls, result.unsupportedControls, result.liveBlockers]) {
    if (typeof value === 'object') assert.equal(Object.isFrozen(value), true);
  }
  assert.throws(() => { result.liveDispatchEligible = true; }, TypeError);
  assert.throws(() => { result.capabilities.authority.publisher = true; }, TypeError);
  assert.throws(() => result.liveBlockers.pop(), TypeError);
});

test('offline assessment has no execution imports and leaves default runtime selection untouched', () => {
  const source = readFileSync(new URL('../src/lib/copilot-worker-capabilities.mjs', import.meta.url), 'utf8');
  assert.doesNotMatch(source, /^\s*import\b|import\s*\(|process\.|fetch\s*\(/m);
  for (const file of ['../src/cli.mjs', '../src/runtime/loop.mjs']) {
    const runtime = readFileSync(new URL(file, import.meta.url), 'utf8');
    assert.doesNotMatch(runtime, /copilot-worker-capabilities/);
  }
});
