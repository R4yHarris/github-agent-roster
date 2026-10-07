import { test } from 'node:test';
import assert from 'node:assert/strict';
import { resolveRetentionConfig } from '../src/lib/config.mjs';
import { RETENTION_POLICIES } from '../src/lib/paths.mjs';
import { evaluateRetention } from '../src/lib/repo-state.mjs';

test('resolveRetentionConfig: empty env → defaults from RETENTION_POLICIES', () => {
  const cfg = resolveRetentionConfig({});
  assert.equal(cfg.optOut, false);
  for (const scope of ['machine', 'repo', 'worktree', 'run']) {
    assert.equal(cfg.windowsMs[scope], RETENTION_POLICIES[scope].defaultWindowMs);
  }
});

test('resolveRetentionConfig: ROSTER_RETENTION_OPT_OUT=true sets optOut', () => {
  const cfg = resolveRetentionConfig({ ROSTER_RETENTION_OPT_OUT: 'true' });
  assert.equal(cfg.optOut, true);
});

test('resolveRetentionConfig: ROSTER_RETENTION_OPT_OUT=false → optOut false', () => {
  const cfg = resolveRetentionConfig({ ROSTER_RETENTION_OPT_OUT: 'false' });
  assert.equal(cfg.optOut, false);
});

test('resolveRetentionConfig: ROSTER_RETENTION_OPT_OUT invalid value throws', () => {
  assert.throws(() => resolveRetentionConfig({ ROSTER_RETENTION_OPT_OUT: 'yes' }),
    /ROSTER_RETENTION_OPT_OUT/);
});

test('resolveRetentionConfig: per-scope window override', () => {
  const cfg = resolveRetentionConfig({ ROSTER_RETENTION_WINDOW_MS_REPO: '12345' });
  assert.equal(cfg.windowsMs.repo, 12345);
  assert.equal(cfg.windowsMs.machine, RETENTION_POLICIES.machine.defaultWindowMs);
  assert.equal(cfg.windowsMs.worktree, RETENTION_POLICIES.worktree.defaultWindowMs);
  assert.equal(cfg.windowsMs.run, RETENTION_POLICIES.run.defaultWindowMs);
});

test('resolveRetentionConfig: rejects non-positive / non-integer window override', () => {
  assert.throws(() => resolveRetentionConfig({ ROSTER_RETENTION_WINDOW_MS_RUN: '0' }),
    /ROSTER_RETENTION_WINDOW_MS_RUN/);
  assert.throws(() => resolveRetentionConfig({ ROSTER_RETENTION_WINDOW_MS_RUN: '1.5' }),
    /ROSTER_RETENTION_WINDOW_MS_RUN/);
  assert.throws(() => resolveRetentionConfig({ ROSTER_RETENTION_WINDOW_MS_RUN: 'abc' }),
    /ROSTER_RETENTION_WINDOW_MS_RUN/);
});

test('opt-out: with optOut set, evaluateRetention keeps every scope with reason opted-out', () => {
  const cfg = resolveRetentionConfig({ ROSTER_RETENTION_OPT_OUT: 'true' });
  assert.equal(cfg.optOut, true);
  const nowMs = 9_999_999_999; // far past every default window
  for (const scope of Object.keys(RETENTION_POLICIES)) {
    const result = evaluateRetention(RETENTION_POLICIES[scope], {
      nowMs,
      windowMs: cfg.windowsMs[scope],
      optOut: cfg.optOut,
    });
    assert.deepEqual(result, { keep: true, reason: 'opted-out' });
  }
});

test('custom window: a user-set window override is honored by evaluateRetention', () => {
  const cfg = resolveRetentionConfig({ ROSTER_RETENTION_WINDOW_MS_RUN: '1000' });
  const policy = RETENTION_POLICIES.run;
  // Age 2000ms > 1000ms window → expired
  const expired = evaluateRetention(policy, {
    nowMs: 2000, windowMs: cfg.windowsMs.run, optOut: cfg.optOut, createdAtMs: 0,
  });
  assert.equal(expired.keep, false);
  assert.equal(expired.reason, 'expired');
  // Default window would be 1 day (86_400_000ms), so age 2000ms would have been kept:
  const defaultResult = evaluateRetention(policy, {
    nowMs: 2000, windowMs: policy.defaultWindowMs, optOut: false, createdAtMs: 0,
  });
  assert.equal(defaultResult.keep, true);
});
