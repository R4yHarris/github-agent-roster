import assert from 'node:assert/strict';
import { test } from 'node:test';
import { classifyStrandedWork, defaultStaleThresholdMs } from '../src/lib/stranded.mjs';

const now = Date.parse('2026-10-04T20:00:00.000Z');
const assigned = (number, state = 'OPEN') => ({ number, state,
  assignees: [{ login: 'worker' }], labels: [{ name: 'in-progress' }] });

test('classifies only open assigned issues without mutating input or using labels as assignment', () => {
  const issues = [assigned(1), assigned(2), assigned(3), assigned(4, 'CLOSED'),
    { ...assigned(5), assignees: [] }, assigned(6, 'open')];
  const input = { issues, branches: ['issue-2', 'issue-3'],
    worktrees: [{ branch: 'issue-6', path: 'unused' }],
    heartbeats: [{ issue: 3, timestamp: now }, { issue: 6, timestamp: now - 1000 }],
    now, thresholdMs: 1000 };
  const before = structuredClone(input);
  assert.deepEqual(classifyStrandedWork(input), [
    { issue: issues[0], branch: 'issue-1', status: 'stranded', lastHeartbeat: null },
    { issue: issues[1], branch: 'issue-2', status: 'stale', lastHeartbeat: null },
    { issue: issues[2], branch: 'issue-3', status: 'healthy', lastHeartbeat: now },
    { issue: issues[5], branch: 'issue-6', status: 'healthy', lastHeartbeat: now - 1000 },
  ]);
  assert.deepEqual(input, before);
});

test('uses newest heartbeat across seats and an inclusive threshold with Date and ISO inputs', () => {
  const base = { issues: [assigned(1)], branches: ['issue-1'], now: new Date(now), thresholdMs: 1000 };
  const heartbeats = [
    { issue: 1, timestamp: new Date(now - 1000).toISOString() },
    { issue: 1, timestamp: new Date(now - 2000) },
    { issue: 2, timestamp: now },
  ];
  assert.equal(classifyStrandedWork({ ...base, heartbeats })[0].status, 'healthy');
  assert.equal(classifyStrandedWork({ ...base, now: now + 1, heartbeats })[0].status, 'stale');
  assert.equal(classifyStrandedWork({ ...base, branches: ['issue-10'], heartbeats })[0].status, 'stranded');
});

test('missing or old heartbeat reports stale, not a fabricated dead worker lock', () => {
  const base = { issues: [assigned(1)], branches: ['issue-1'], now };
  assert.equal(defaultStaleThresholdMs, 30 * 60 * 1000);
  assert.equal(classifyStrandedWork(base)[0].status, 'stale');
  const result = classifyStrandedWork({ ...base,
    heartbeats: [{ issue: 1, timestamp: now - defaultStaleThresholdMs - 1 }] })[0];
  assert.equal(result.status, 'stale');
  assert.equal(Object.hasOwn(result, 'lock'), false);
  assert.equal(Object.hasOwn(result, 'pid'), false);
});

test('invalid evidence fails explicitly instead of producing healthy defaults', () => {
  const base = { issues: [assigned(1)], now };
  for (const invalid of [
    { issues: null }, { worktrees: null }, { branches: null }, { heartbeats: null },
    { now: undefined }, { now: 'invalid' }, { now: NaN },
    { thresholdMs: 0 }, { thresholdMs: -1 }, { thresholdMs: 1.5 },
    { branches: [null] }, { worktrees: [{}] },
    { issues: [{ ...assigned(1), state: 'UNKNOWN' }] },
    { issues: [{ ...assigned(1), number: 0 }] }, { issues: [{ number: 1, state: 'OPEN' }] },
    { heartbeats: [{ issue: 0, timestamp: now }] },
    { heartbeats: [{ issue: 1, timestamp: 'invalid' }] },
  ]) assert.throws(() => classifyStrandedWork({ ...base, ...invalid }), TypeError);
  assert.deepEqual(classifyStrandedWork({ issues: [], now }), []);
});
