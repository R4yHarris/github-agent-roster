import test from 'node:test';
import assert from 'node:assert/strict';
import { parallelLimit, runReadyWaves } from '../src/lib/wave-scheduler.mjs';

const row = (issue, state = 'todo') => ({ issue, wave: 1, state });
const passed = () => ({ review: { verdict: 'pass' } });

test('parallel limit validates CLI and API values', () => {
  assert.equal(parallelLimit(), 1);
  assert.equal(parallelLimit('2'), 2);
  for (const value of [0, -1, 1.5, '0', '2x', null, Number.MAX_SAFE_INTEGER + 1]) {
    assert.throws(() => parallelLimit(value), /positive safe integer/);
  }
});

test('ready waves overlap, stay bounded by capacity, and each is attempted once', async () => {
  let active = 0;
  let peak = 0;
  const started = [];
  const finished = [];
  const events = [];
  let release;
  const bothStarted = new Promise((resolve) => { release = resolve; });
  const result = await runReadyWaves({ parallel: 5, capacity: 2,
    readBoard: async () => [row(1), row(2), row(3)],
    onState: (event) => events.push(event),
    runChild: async ({ issue }) => {
      started.push(issue);
      peak = Math.max(peak, ++active);
      if (started.length === 2) release();
      if (issue < 3) await bothStarted;
      finished.push(issue);
      active--;
      return passed();
    },
  });
  assert.equal(peak, 2);
  assert.equal(result.parallel, 2);
  assert.equal(result.failed, false);
  assert.deepEqual(started, [1, 2, 3]);
  assert.deepEqual(finished, [1, 2, 3]);
  assert.equal(events.filter((event) => event.type === 'wave-end').length, 3);
});

test('board refresh admits dependencies only after closure, not after a reviewed result', async () => {
  let closed = false;
  const started = [];
  const result = await runReadyWaves({ parallel: 2, capacity: 2,
    readBoard: async () => [row(1, closed ? 'done' : 'todo'), row(2, closed ? 'todo' : 'blocked')],
    runChild: async ({ issue }) => { started.push(issue); closed = true; return passed(); },
  });
  assert.deepEqual(started, [1, 2]);
  assert.equal(result.children.length, 2);
  const reviewed = await runReadyWaves({ parallel: 2, capacity: 2,
    readBoard: async () => [row(1), row(2, 'blocked')], runChild: async () => passed(),
  });
  assert.deepEqual(reviewed.children.map(({ issue }) => issue), [1]);
});

test('a crashed child does not stop siblings and preserves rejection evidence', async () => {
  const failure = new Error('child crashed');
  const result = await runReadyWaves({ parallel: 2, capacity: 2,
    readBoard: async () => [row(1), row(2), row(3)],
    runChild: async ({ issue }) => { if (issue === 1) throw failure; return passed(); },
  });
  assert.equal(result.failed, true);
  assert.equal(result.children[0].error, failure);
  assert.deepEqual(result.children.map(({ status }) => status), ['rejected', 'fulfilled', 'fulfilled']);
});

test('parallel one preserves sequential order and cancellation prevents further starts', async () => {
  const started = [];
  const controller = new AbortController();
  const options = { parallel: 1, capacity: 3, readBoard: async () => [row(1), row(2)],
    runChild: async ({ issue }) => { started.push(issue); return passed(); } };
  await runReadyWaves(options);
  assert.deepEqual(started, [1, 2]);
  started.length = 0;
  await assert.rejects(runReadyWaves({ ...options, signal: controller.signal,
    runChild: async ({ issue }) => { started.push(issue); controller.abort(); return passed(); },
  }), /cancel|abort/i);
  assert.deepEqual(started, [1]);
});

test('a failed board refresh retains completed child evidence and reports the failure', async () => {
  let reads = 0;
  const boardError = new Error('board offline');
  const result = await runReadyWaves({ parallel: 2, capacity: 2,
    readBoard: async () => { if (reads++) throw boardError; return [row(1)]; },
    runChild: async () => passed(),
  });
  assert.equal(result.failed, true);
  assert.equal(result.boardError, boardError);
  assert.equal(result.children[0].status, 'fulfilled');
});
