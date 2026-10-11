import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';
import test from 'node:test';
import { parseConfig, formatConfig } from '../src/lib/config.mjs';
import { throwIfCancelled } from '../src/runtime/cancel.mjs';
import { TaskTimeoutError, withTaskDeadline } from '../src/runtime/task-deadline.mjs';
import { withRequestTimeout } from '../src/llm/request.mjs';
import { example } from './helpers/builtin.mjs';

test('whole-task timeout is opt-in, strictly bounded and survives private config round trips', () => {
  assert.equal(parseConfig(example).seat.task_timeout_ms, undefined);
  const source = example.replace(/\r\n/g, '\n').replace('seat:\n', 'seat:\n  task_timeout_ms: 60000\n');
  const config = parseConfig(source);
  assert.equal(config.seat.task_timeout_ms, 60000);
  assert.equal(parseConfig(formatConfig(config)).seat.task_timeout_ms, 60000);
  for (const value of ['0', '-1', '1.5', '2147483648', '"1000"']) {
    assert.throws(() => parseConfig(source.replace('task_timeout_ms: 60000', `task_timeout_ms: ${value}`)),
      /task_timeout_ms/);
  }
});

test('one deadline spans sequential operations, waits for cleanup and rejects late success', async () => {
  let expire;
  let scheduled = 0;
  let cleared = 0;
  let stopped = false;
  await assert.rejects(withTaskDeadline(async (signal) => {
    assert.equal(signal.aborted, false);
    await Promise.resolve();
    await Promise.resolve();
    expire();
    assert.throws(() => throwIfCancelled(signal), TaskTimeoutError);
    await delay(5);
    stopped = true;
    return { pass: true };
  }, { timeoutMs: 1000, schedule: (callback, ms) => {
    assert.equal(ms, 1000);
    scheduled += 1;
    expire = callback;
    return 123;
  }, unschedule: (timer) => { assert.equal(timer, 123); cleared += 1; } }),
  { code: 'ROSTER_TASK_TIMEOUT', timeoutMs: 1000 });
  assert.equal(stopped, true);
  assert.equal(scheduled, 1);
  assert.equal(cleared, 1);
});

test('uncapped behavior and explicit operator cancellation retain their existing meaning', async () => {
  const original = new AbortController();
  assert.equal(await withTaskDeadline(async (signal) => {
    assert.equal(signal, original.signal);
    return 42;
  }, { signal: original.signal, schedule: () => assert.fail('Uncapped task must not schedule a timer') }), 42);
  await assert.rejects(withTaskDeadline(async (signal) => {
    original.abort();
    throwIfCancelled(signal);
  }, { signal: original.signal, timeoutMs: 1000 }), { code: 'ROSTER_CANCELLED' });
  await assert.rejects(withTaskDeadline(() => assert.fail('Cancelled task must not start'),
    { signal: original.signal, timeoutMs: 1000 }), { code: 'ROSTER_CANCELLED' });
});

test('separate child task deadlines do not cancel a healthy sibling or its parent', async () => {
  const parent = new AbortController();
  let expire;
  const stopped = withTaskDeadline(async (signal) => {
    await Promise.resolve();
    expire();
    throwIfCancelled(signal);
  }, { signal: parent.signal, timeoutMs: 100,
    schedule: (callback) => { expire = callback; return 1; }, unschedule: () => {} });
  const healthy = withTaskDeadline(async (signal) => {
    await delay(5);
    throwIfCancelled(signal);
    return 'delivered';
  }, { signal: parent.signal, timeoutMs: 1000 });
  await assert.rejects(stopped, { code: 'ROSTER_TASK_TIMEOUT' });
  assert.equal(await healthy, 'delivered');
  assert.equal(parent.signal.aborted, false);
});

test('task expiry interrupts a pending model request without being mislabeled as its HTTP timeout', async () => {
  let requestStopped = false;
  await assert.rejects(withTaskDeadline((signal) => withRequestTimeout((requestSignal) =>
    new Promise((resolve) => requestSignal.addEventListener('abort', () => {
      requestStopped = true;
      resolve({});
    }, { once: true })), {
    host: 'localhost', local: true, timeoutMs: 1000,
    retryCommand: 'roster run --issue 42', signal,
  }), { timeoutMs: 20 }), { code: 'ROSTER_TASK_TIMEOUT', timeoutMs: 20 });
  assert.equal(requestStopped, true);
});

test('task timeout preserves attached failure evidence without a circular error cause', async () => {
  let expire;
  const result = { excellence: { pass: false } };
  await assert.rejects(withTaskDeadline(async (signal) => {
    expire();
    const failure = new Error('Coder stopped', { cause: signal.reason });
    failure.result = result;
    throw failure;
  }, { timeoutMs: 1000, schedule: (callback) => { expire = callback; return 1; }, unschedule: () => {} }),
  (error) => {
    assert.equal(error.code, 'ROSTER_TASK_TIMEOUT');
    assert.equal(error.result, result);
    assert.equal(error.cause, undefined);
    return true;
  });
});
