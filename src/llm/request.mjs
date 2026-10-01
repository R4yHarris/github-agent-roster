import { isIP } from 'node:net';
import { performance } from 'node:perf_hooks';

export const localRequestTimeoutMs = 1_200_000;
export const cloudRequestTimeoutMs = 120_000;
export const waitingIntervalMs = 30_000;

export class ChatError extends Error {
  constructor(message, category = 'response') {
    super(message);
    this.category = category;
  }
}

export function isLocalLlmHost(hostname) {
  const host = hostname.toLowerCase().replace(/^\[|\]$/g, '');
  if (host === 'localhost' || host === '::1') return true;
  if (isIP(host) !== 4) return false;
  const [first, second] = host.split('.').map(Number);
  return first === 127 || first === 10 || first === 192 && second === 168 ||
    first === 172 && second >= 16 && second <= 31;
}

export function validateRequestTimeout(value, field = 'llm.request_timeout_ms') {
  if (!Number.isSafeInteger(value) || value < 1 || value > 2_147_483_647) {
    throw new TypeError(`${field} must be an integer between 1 and 2147483647.`);
  }
  return value;
}

export function resolveRequestTimeout(llm) {
  if (llm.request_timeout_ms !== undefined) return validateRequestTimeout(llm.request_timeout_ms);
  if (llm.timeout_ms !== undefined) return validateRequestTimeout(llm.timeout_ms, 'llm.timeout_ms');
  return isLocalLlmHost(new URL(llm.base_url).hostname) ? localRequestTimeoutMs : cloudRequestTimeoutMs;
}

export function retryCommandForTask(task) {
  const issue = /^issue-([1-9]\d*)$/.exec(task ?? '');
  return issue ? `roster run --issue ${issue[1]}` : 'roster run --seat coder --runtime builtin';
}

export function validateRetryCommand(command) {
  if (command !== undefined && !/^(?:roster run --issue [1-9]\d*(?: --auto-model)?|roster run --seat coder --runtime builtin|roster doctor --warm)$/.test(command)) {
    throw new TypeError('LLM retry command must name a supported Roster operation');
  }
  return command;
}

export function timeoutHint({ host, timeoutMs, local, retryCommand }) {
  return `The LLM request timed out after ${timeoutMs / 1000}s at host=${host}. ` +
    (local ? 'Cold-start: the host may still be warming; Spark/SGLang can take up to 15m. ' : '') +
    'This is an endpoint timeout, not a bad TASK. ' +
    (retryCommand ? `Retry: ${retryCommand}` : 'Retry the same request.');
}

export class LlmTimeoutError extends ChatError {
  constructor(options) {
    super(timeoutHint(options), 'timeout');
    this.code = 'ROSTER_LLM_TIMEOUT';
  }
}

export function isLlmTimeout(error) {
  const seen = new Set();
  while (error instanceof Error && !seen.has(error)) {
    if (error.code === 'ROSTER_LLM_TIMEOUT') return true;
    seen.add(error);
    error = error.cause;
  }
  return false;
}

export async function withRequestTimeout(operation, {
  host, local, timeoutMs, retryCommand, onWaiting, clock = () => performance.now(),
}) {
  validateRequestTimeout(timeoutMs);
  validateRetryCommand(retryCommand);
  if (typeof clock !== 'function' || onWaiting !== undefined && typeof onWaiting !== 'function') {
    throw new TypeError('Request clock and waiting observer must be functions');
  }
  const controller = new AbortController();
  const started = clock();
  let active = true;
  let pending = Promise.resolve();
  let timer;
  let interval;
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(() => {
      reject(new LlmTimeoutError({ host, timeoutMs, local, retryCommand }));
      controller.abort();
    }, timeoutMs);
  });
  const observerFailure = new Promise((_, reject) => {
    if (!onWaiting) return;
    interval = setInterval(() => {
      const elapsedSeconds = Math.max(0, Math.floor((clock() - started) / 1000));
      pending = pending.then(() => active ? onWaiting({ host, elapsedSeconds, local }) : undefined);
      pending.catch((error) => {
        reject(error);
        controller.abort();
      });
    }, waitingIntervalMs);
  });
  try {
    return await Promise.race([Promise.resolve().then(() => operation(controller.signal)), deadline, observerFailure]);
  } finally {
    active = false;
    clearTimeout(timer);
    clearInterval(interval);
    controller.abort();
    await pending;
  }
}
