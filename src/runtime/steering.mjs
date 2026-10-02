import { isLlmTimeout } from '../llm/request.mjs';
import { isRunCancelled, RunCancelledError, throwIfCancelled } from './cancel.mjs';

export class SteeringInterrupt extends Error {
  code = 'ROSTER_STEERING';

  constructor() {
    super('Coder request interrupted for human steering.');
  }
}

export function createSteeringControl({ signal } = {}) {
  if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError('Steering signal must be an AbortSignal');
  let current = null;
  let instruction = null;
  return {
    get waiting() { return current !== null && !current.steered; },
    steer(text) {
      if (typeof text !== 'string' || !text.trim() || text.length > 4096) {
        throw new TypeError('Steering needs 1-4096 characters of human instruction.');
      }
      throwIfCancelled(signal);
      if (!current || current.steered) throw new Error('The coder is not awaiting a model call; wait for drafting before /steer.');
      instruction = text.trim();
      current.steered = true;
      current.controller.abort(new RunCancelledError());
    },
    take() {
      const next = instruction;
      instruction = null;
      return next;
    },
    async request(operation) {
      throwIfCancelled(signal);
      if (current) throw new Error('Only one coder model request can be active.');
      const request = { controller: new AbortController(), steered: false };
      current = request;
      const abort = () => request.controller.abort(new RunCancelledError());
      signal?.addEventListener('abort', abort, { once: true });
      try {
        const result = await operation(request.controller.signal);
        throwIfCancelled(signal);
        if (request.steered) throw new SteeringInterrupt();
        return result;
      } catch (error) {
        throwIfCancelled(signal);
        if (request.steered && !isLlmTimeout(error) &&
            (isRunCancelled(error) || error instanceof SteeringInterrupt)) throw new SteeringInterrupt();
        throw error;
      } finally {
        signal?.removeEventListener('abort', abort);
        if (current === request) current = null;
      }
    },
    dispose() {
      instruction = null;
      current?.controller.abort(new RunCancelledError());
    },
  };
}
