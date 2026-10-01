export class RunCancelledError extends Error {
  code = 'ROSTER_CANCELLED';

  constructor() {
    super('Run cancelled.');
  }
}

export function throwIfCancelled(signal) {
  if (signal?.aborted) throw new RunCancelledError();
}

export function isRunCancelled(error) {
  const seen = new Set();
  while (error instanceof Error && !seen.has(error)) {
    if (error.code === 'ROSTER_CANCELLED') return true;
    seen.add(error);
    error = error.cause;
  }
  return false;
}
