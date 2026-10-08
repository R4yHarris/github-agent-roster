import { throwIfCancelled } from '../runtime/cancel.mjs';

export function parallelLimit(value = 1) {
  const number = typeof value === 'string' && /^[1-9]\d*$/.test(value) ? Number(value) : value;
  if (!Number.isSafeInteger(number) || number < 1) throw new TypeError('--parallel must be a positive safe integer');
  return number;
}

export async function runReadyWaves({ parallel, capacity, readBoard, runChild, signal, onState }) {
  const limit = Math.min(parallelLimit(parallel), parallelLimit(capacity));
  const children = [];
  const attempted = new Set();
  let waves;
  for (;;) {
    throwIfCancelled(signal);
    try {
      waves = await readBoard();
    } catch (boardError) {
      if (!children.length) throw boardError;
      return { children, waves, parallel: limit, failed: true, boardError };
    }
    const ready = waves.filter((row) => row.issue && row.state === 'todo' && !attempted.has(row.issue)).slice(0, limit);
    if (!ready.length) break;
    const settled = await Promise.allSettled(ready.map(async (row) => {
      attempted.add(row.issue);
      await onState?.({ type: 'wave-start', issue: row.issue, wave: row.wave });
      let state = 'failed';
      try {
        const result = await runChild(row);
        if (!result?.failed && result?.review?.verdict === 'pass') state = 'review';
        return result;
      } finally {
        await onState?.({ type: 'wave-end', issue: row.issue, wave: row.wave, state });
      }
    }));
    for (const [index, outcome] of settled.entries()) {
      const issue = ready[index].issue;
      children.push(outcome.status === 'fulfilled'
        ? { issue, status: 'fulfilled', result: outcome.value }
        : { issue, status: 'rejected', error: outcome.reason });
    }
  }
  return { children, waves, parallel: limit,
    failed: children.some((child) => child.status === 'rejected' || child.result?.failed ||
      child.result?.review?.verdict !== 'pass') };
}
