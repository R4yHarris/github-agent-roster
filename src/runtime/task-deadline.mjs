import { RunCancelledError, throwIfCancelled } from './cancel.mjs';

export class TaskTimeoutError extends Error {
  code = 'ROSTER_TASK_TIMEOUT';

  constructor(timeoutMs) {
    super(`Task wall-clock budget exhausted after ${timeoutMs} ms; work was cancelled and remains unverified.`);
    this.timeoutMs = timeoutMs;
  }
}

export function validateTaskTimeout(timeoutMs) {
  if (timeoutMs !== undefined &&
      (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 2_147_483_647)) {
    throw new TypeError('seat.task_timeout_ms must be an integer between 1 and 2147483647');
  }
  return timeoutMs;
}

export async function withTaskDeadline(operation, {
  timeoutMs, signal, schedule = setTimeout, unschedule = clearTimeout,
} = {}) {
  validateTaskTimeout(timeoutMs);
  if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError('Run signal must be an AbortSignal');
  throwIfCancelled(signal);
  if (timeoutMs === undefined) return operation(signal);
  const controller = new AbortController();
  const abort = () => controller.abort(signal.reason ?? new RunCancelledError());
  signal?.addEventListener('abort', abort, { once: true });
  const timer = schedule(() => controller.abort(new TaskTimeoutError(timeoutMs)), timeoutMs);
  try {
    // Keep ownership until the cancelled operation has finished its cleanup.
    const result = await operation(controller.signal);
    throwIfCancelled(controller.signal);
    return result;
  } catch (error) {
    if (controller.signal.reason instanceof TaskTimeoutError) {
      const timeout = controller.signal.reason;
      if (error !== timeout && error?.result) timeout.result = error.result;
      throw timeout;
    }
    throw error;
  } finally {
    unschedule(timer);
    signal?.removeEventListener('abort', abort);
  }
}
