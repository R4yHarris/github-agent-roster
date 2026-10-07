import { test } from 'node:test';
import assert from 'node:assert/strict';
import { evaluateRetention } from '../src/lib/repo-state.mjs';

const policy = { scope: 'machine', root: 'machine-root', defaultWindowMs: 86_400_000 };

test('evaluateRetention: opt-out always keeps with reason opted-out', () => {
  const result = evaluateRetention(policy, { nowMs: 1_000_000, windowMs: 60_000, optOut: true });
  assert.deepEqual(result, { keep: true, reason: 'opted-out' });
});

test('evaluateRetention: expired (nowMs > expiresAtMs) is not kept', () => {
  // age = nowMs - createdAtMs = 200_000 - 50_000 = 150_000 > windowMs=100_000 → expired
  const expired = evaluateRetention(policy, {
    nowMs: 200_000, windowMs: 100_000, optOut: false, createdAtMs: 50_000,
  });
  assert.equal(expired.keep, false);
  assert.equal(expired.reason, 'expired');
  assert.equal(expired.expiresAtMs, 150_000);
});

test('evaluateRetention: fresh (age < windowMs) is kept with reason within-window', () => {
  const result = evaluateRetention(policy, {
    nowMs: 200_000, windowMs: 100_000, optOut: false, createdAtMs: 150_000,
  });
  assert.equal(result.keep, true);
  assert.equal(result.reason, 'within-window');
  assert.equal(result.expiresAtMs, 250_000);
});

test('evaluateRetention: exact boundary (age === windowMs) keeps (strict > rule)', () => {
  // age = 200_000 - 100_000 = 100_000 === windowMs; strict > does not fire.
  const result = evaluateRetention(policy, {
    nowMs: 200_000, windowMs: 100_000, optOut: false, createdAtMs: 100_000,
  });
  assert.equal(result.keep, true, 'age == windowMs → not expired (strict >)');
  assert.equal(result.expiresAtMs, 200_000);
});

test('evaluateRetention: one ms past boundary is expired', () => {
  // age = 200_001 - 100_000 = 100_001 > windowMs=100_000 → expired
  const result = evaluateRetention(policy, {
    nowMs: 200_001, windowMs: 100_000, optOut: false, createdAtMs: 100_000,
  });
  assert.equal(result.keep, false);
  assert.equal(result.reason, 'expired');
});

test('evaluateRetention: an unknown age or non-boolean opt-out fails closed instead of expiring', () => {
  for (const createdAtMs of [undefined, Number.NaN, -1, '100']) {
    assert.throws(() => evaluateRetention(policy, { nowMs: 200, windowMs: 100, optOut: false, createdAtMs }),
      /createdAtMs/);
  }
  assert.throws(() => evaluateRetention(policy, { nowMs: 200, windowMs: 100, optOut: 'true', createdAtMs: 0 }),
    /optOut/);
  assert.deepEqual(evaluateRetention(policy, { nowMs: 200, windowMs: 100, optOut: true }),
    { keep: true, reason: 'opted-out' });
});

test('evaluateRetention: determinism - identical inputs yield identical outputs', () => {
  const inputs = { nowMs: 42, windowMs: 1000, optOut: false, createdAtMs: 0 };
  const a = evaluateRetention(policy, inputs);
  const b = evaluateRetention(policy, inputs);
  assert.deepEqual(a, b);
  const c = evaluateRetention(policy, { ...inputs, optOut: true });
  const d = evaluateRetention(policy, { ...inputs, optOut: true });
  assert.deepEqual(c, d);
  assert.deepEqual(c, { keep: true, reason: 'opted-out' });
  // Different scopes: still deterministic for the same inputs
  const p2 = { scope: 'run', root: 'run-root', defaultWindowMs: 1 };
  const e = evaluateRetention(p2, { nowMs: 42, windowMs: 10, optOut: false, createdAtMs: 0 });
  const f = evaluateRetention(p2, { nowMs: 42, windowMs: 10, optOut: false, createdAtMs: 0 });
  assert.deepEqual(e, f);
});

test('evaluateRetention: rejects malformed inputs with typed errors', () => {
  assert.throws(
    () => evaluateRetention(null, { nowMs: 0, windowMs: 1, optOut: false }),
    /retention policy/i);
  assert.throws(
    () => evaluateRetention(policy, { nowMs: Number.NaN, windowMs: 1, optOut: false }),
    /nowMs/i);
  assert.throws(
    () => evaluateRetention(policy, { nowMs: 0, windowMs: 0, optOut: false }),
    /windowMs/i);
});
