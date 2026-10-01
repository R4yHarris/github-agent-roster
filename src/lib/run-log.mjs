import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { ensureLocalPath } from './paths.mjs';
import { redactSecrets } from '../runtime/memory.mjs';
import { timeoutHint, validateRequestTimeout, validateRetryCommand } from '../llm/request.mjs';
import { createDebugLog } from './debug-log.mjs';

const seats = ['planner', 'coder', 'reviewer'];
const tools = ['read_file', 'write_file', 'list_dir', 'run_test', 'search_text'];
const artifacts = ['RECIPE.yml', 'TASK.md', 'PLAN.md', 'ESTIMATE.md', 'RESULT.md', 'REVIEW.md'];
const httpErrors = ['authentication', 'network', 'timeout', 'http', 'response', 'abort'];
const maximumLineBytes = 2048;
const seatActions = {
  planner: 'Writing the plan: outcome, allowed files, and checks.',
  coder: 'Drafting the change.',
  reviewer: 'Checking the diff against the task.',
};

export class RunLogError extends Error {
  code = 'ROSTER_RUN_LOG';
}

function logPath(repoRoot, session) {
  if (typeof session !== 'string' || !/^[A-Za-z0-9._-]{1,64}$/.test(session)) {
    throw new TypeError('Run log session must be an opaque identifier of at most 64 characters');
  }
  return path.resolve(repoRoot, '.roster', 'runs', `${session}.log`);
}

function errorClass(error) {
  if (error?.code === 'ROSTER_RUN_LOG') return 'RunLogError';
  if (error instanceof TypeError) return 'TypeError';
  if (error instanceof RangeError) return 'RangeError';
  if (error?.name === 'AbortError') return 'AbortError';
  return 'Error';
}

export async function createRunLog({
  repoRoot, session, env = process.env, apiKeyEnv = 'ROSTER_API_KEY',
  errorOutput = process.stderr, now = () => new Date(), clock = () => performance.now(),
  debug = createDebugLog({ env }), issue = null,
  observe,
}) {
  if (typeof repoRoot !== 'string' || typeof errorOutput?.write !== 'function' ||
      typeof now !== 'function' || typeof clock !== 'function') {
    throw new TypeError('Run log requires a repository root, stderr writer, and clocks');
  }
  if (observe !== undefined && typeof observe !== 'function') throw new TypeError('Seat observer must be a function');
  const safe = (value) => redactSecrets(value, { env, apiKeyEnv }).replace(/[\x00-\x1f\x7f]/g, '?');
  if (safe(session) !== session) throw new TypeError('Run log session must not contain credentials');
  const file = logPath(repoRoot, session);
  await ensureLocalPath(file, repoRoot);
  await fs.mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
  await ensureLocalPath(file, repoRoot);
  let pending = Promise.resolve();

  function append(message, human = null) {
    const write = pending.then(async () => {
      const line = `${now().toISOString()} ${safe(message)}\n`;
      if (Buffer.byteLength(line) > maximumLineBytes) throw new RunLogError('Run log metadata line exceeds 2 KiB');
      await ensureLocalPath(file, repoRoot);
      const handle = await fs.open(file, constants.O_WRONLY | constants.O_APPEND | constants.O_CREAT |
        (constants.O_NOFOLLOW ?? 0), 0o600);
      try {
        const entry = await handle.stat();
        if (!entry.isFile() || entry.nlink !== 1) throw new RunLogError('Run log must be a regular, single-link file');
        await handle.writeFile(line, 'utf8');
      } finally {
        await handle.close();
      }
      if (human !== null) errorOutput.write(`${safe(human)}\n`);
    }).catch((error) => {
      if (error instanceof RunLogError) throw error;
      throw new RunLogError(`Could not write live run log (${typeof error.code === 'string' &&
        /^[A-Z0-9_]+$/.test(error.code) ? error.code : errorClass(error)})`);
    });
    pending = write;
    return write;
  }

  function humanEventText(name, event) {
    if (event.type === 'finish-reason') return event.retry
      ? event.withoutReasoning ? 'Response truncated. Retrying without reasoning.' : 'Response truncated. Retrying.'
      : `Unsupported LLM finish reason: ${safe(event.reason)}.`;
    if (event.type === 'contracts-uninitialized') return 'Contracts submodule was not initialized';
    if (event.type === 'test-repair') return `Tests failed. Repair ${event.attempt} of ${event.budget}.`;
    if (event.type === 'http' && event.phase === 'start') {
      return name === 'coder' && event.effort
        ? `Drafting at ${event.effort} effort. Model prior: ${event.modelPrior ?? 'unknown'}.`
        : seatActions[name];
    }
    if (event.type === 'waiting' && event.elapsedSeconds > 30) {
      return 'Still waiting on the model. Local hardware can take minutes after idle.';
    }
    if (event.type === 'timeout') return 'The model did not answer in time. It may still be waking.';
    if (event.type !== 'tool') return null;
    const location = typeof event.path === 'string' ? safe(event.path).slice(0, 512) : 'the requested file';
    switch (event.name) {
      case 'read_file': return `Reading ${location} before editing.`;
      case 'write_file': return `Saving ${location}.`;
      case 'run_test': return 'Running tests.';
      case 'list_dir': return `Listing ${location}.`;
      case 'search_text': return 'Finding the relevant text.';
      default: throw new TypeError('Invalid live tool event');
    }
  }

  function eventText(event) {
    switch (event.type) {
      case 'completion':
        if (![null, 'stop', 'tool_calls'].includes(event.reason)) throw new TypeError('Invalid live completion reason');
        return `completion finish_reason=${JSON.stringify(event.reason)}`;
      case 'finish-reason':
        if (typeof event.reason !== 'string' || !event.reason || event.reason.length > 128 ||
            /[\x00-\x1f\x7f]/.test(event.reason) || typeof event.retry !== 'boolean') {
          throw new TypeError('Invalid live finish reason');
        }
        return `finish_reason=${JSON.stringify(safe(event.reason))} retry=${event.retry}` +
          (event.retry ? event.withoutReasoning
            ? ' Response truncated. Retrying without reasoning.' : ' Response truncated. Retrying.' : '');
      case 'contracts-uninitialized': return 'contracts submodule uninitialized';
      case 'test-repair':
        if (event.budget !== 4 || !Number.isInteger(event.attempt) || event.attempt < 1 || event.attempt > event.budget) {
          throw new TypeError('Invalid live test repair event');
        }
        return `test repair ${event.attempt}/${event.budget}`;
      case 'waiting':
      case 'timeout': {
        if (typeof event.host !== 'string' || !/^[A-Za-z0-9.:[\]-]{1,255}$/.test(event.host) ||
            typeof event.local !== 'boolean') throw new TypeError('Invalid live waiting host');
        if (event.type === 'timeout') {
          validateRequestTimeout(event.timeoutMs);
          validateRetryCommand(event.retryCommand);
          return timeoutHint({ ...event, host: safe(event.host) });
        }
        if (!Number.isSafeInteger(event.elapsedSeconds) || event.elapsedSeconds < 0) {
          throw new TypeError('Invalid live waiting duration');
        }
        return `waiting host=${safe(event.host)} elapsed=${event.elapsedSeconds}s` +
          (event.local ? ' cold-start up to 15m' : '');
      }
      case 'model': {
        const model = typeof event.model === 'string' && /^[A-Za-z0-9][A-Za-z0-9._:+/-]{0,239}$/.test(event.model)
          ? safe(event.model) : '[unavailable]';
        const host = typeof event.host === 'string' && /^[A-Za-z0-9.:[\]-]{1,255}$/.test(event.host)
          ? safe(event.host) : '-';
        return `model=${JSON.stringify(model)} host=${JSON.stringify(host)}`;
      }
      case 'http': {
        if (event.modelPrior !== undefined && !['strong', 'standard', 'limited', 'unknown'].includes(event.modelPrior)) {
          throw new TypeError('Invalid live model capability prior');
        }
        if (event.effort !== undefined && !['low', 'medium', 'high', 'max', 'xhigh', 'none'].includes(event.effort)) {
          throw new TypeError('Invalid live reasoning effort');
        }
        if (!['start', 'ok', 'error'].includes(event.phase) ||
            event.phase === 'ok' && (!Number.isInteger(event.status) || event.status < 200 || event.status > 299) ||
            event.phase === 'error' && !httpErrors.includes(event.errorClass)) {
          throw new TypeError('Invalid live HTTP event');
        }
        return `http chat.completions ${event.phase}` +
          (event.status === undefined ? '' : ` status=${event.status}`) +
          (event.phase === 'error' ? ` class=${event.errorClass}` : '');
      }
      case 'tool': {
        if (!tools.includes(event.name)) throw new TypeError('Invalid live tool event');
        const location = typeof event.path === 'string' ? safe(event.path).slice(0, 512) : '[invalid]';
        return `tool ${event.name}` + (event.path === undefined ? '' : ` path=${JSON.stringify(location)}`);
      }
      case 'tool-result':
        if (!tools.includes(event.name) || !['ok', 'error', 'denied'].includes(event.status)) {
          throw new TypeError('Invalid live tool result event');
        }
        return `tool result ${event.name} ${event.status}`;
      case 'wrote':
        if (!artifacts.includes(event.path)) throw new TypeError('Invalid live artifact event');
        return `wrote ${event.path}`;
      case 'implementation':
        if (!['model', 'deterministic-readme'].includes(event.path)) throw new TypeError('Invalid implementation path');
        return `implementation ${event.path}`;
      default:
        throw new TypeError('Unsupported live run event');
    }
  }

  async function seat(name, seatSession, config, operation) {
    if (!seats.includes(name) || typeof operation !== 'function') throw new TypeError('Invalid logged seat');
    logPath(repoRoot, seatSession);
    let mode = config.llm.base_url ? 'llm' : 'stub';
    let modelEvent;
    const onEvent = async (event) => {
      const text = eventText(event);
      await observe?.({ ...event, seat: name });
      await debug.record({ repoRoot, issue, seat: name, event });
      if (event.type === 'tool-result') return;
      if (event.type === 'model') {
        if (text === modelEvent) return;
        modelEvent = text;
      }
      await append(`seat ${name} ${text}`, humanEventText(name, event));
    };
    const started = clock();
    await observe?.({ type: 'seat-start', seat: name, model: mode === 'stub' ? '' : config.llm.model,
      host: mode === 'stub' ? '' : new URL(config.llm.base_url).host,
      effort: config.llm.effort, contextMax: config.llm.context_max });
    await debug.record({ repoRoot, issue, seat: name, event: { type: 'seat-start' } });
    await append(`start seat ${name} session=${safe(seatSession)}`,
      name === 'coder' && mode === 'stub' ? 'Preparing the task summary.' : seatActions[name]);
    await onEvent({ type: 'model', model: mode === 'stub' ? 'builtin-stub' : config.llm.model,
      host: mode === 'stub' ? '-' : new URL(config.llm.base_url).host });
    await append(`seat ${name} mode ${mode}`);
    try {
      const result = await operation(onEvent);
      await observe?.({ type: 'seat-end', seat: name, verdict: result?.verdict,
        contextUsed: result?.response?.usage?.prompt_tokens, model: result?.response?.model });
      const actualMode = result?.mode ?? (typeof result?.queried === 'boolean' ? result.queried ? 'llm' : 'stub' : mode);
      if (actualMode !== mode) {
        mode = actualMode;
        await append(`seat ${name} mode ${mode}`);
      }
      if (result?.error) await append(`seat ${name} error class=Error`);
      return result;
    } catch (error) {
      mode = error?.result?.mode ?? mode;
      await observe?.({ type: 'seat-error', seat: name });
      await debug.record({ repoRoot, issue, seat: name, event: { type: 'seat-error' } });
      await append(`seat ${name} error class=${errorClass(error)}`);
      throw error;
    } finally {
      await debug.record({ repoRoot, issue, seat: name, event: { type: 'seat-end' } });
      await append(`seat ${name} elapsed_ms=${Math.max(0, Math.round(clock() - started))} mode=${mode}`);
    }
  }

  return { path: file, session, seat };
}

export async function readLastRunLog({
  repoRoot, session, env = process.env, apiKeyEnv = 'ROSTER_API_KEY', limit = 1,
}) {
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 200) throw new TypeError('Run log tail limit must be 1-200');
  const file = logPath(repoRoot, session);
  await ensureLocalPath(file, repoRoot);
  let handle;
  try {
    handle = await fs.open(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw new RunLogError('Could not read live run log');
  }
  try {
    const entry = await handle.stat();
    if (!entry.isFile() || entry.nlink !== 1) throw new RunLogError('Run log must be a regular, single-link file');
    const size = Math.min(entry.size, 65_536);
    const buffer = Buffer.alloc(size);
    const { bytesRead } = await handle.read(buffer, 0, size, entry.size - size);
    const text = buffer.subarray(0, bytesRead).toString('utf8');
    const lines = text.split('\n');
    lines.pop();
    const lastLine = lines.at(-1);
    if (!lastLine) return null;
    const parsed = /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z (?:start seat|seat) (planner|coder|reviewer) (.+)$/.exec(lastLine);
    const metadata = /^(?:session=[A-Za-z0-9._[\]-]{1,128}|model="(?:[^"\\]|\\.)*" host="(?:[^"\\]|\\.)*"|mode (?:stub|llm)|waiting host=[A-Za-z0-9.:[\]-]{1,255} elapsed=\d+s(?: cold-start up to 15m)?|The LLM request timed out after \d+(?:\.\d+)?s at host=[A-Za-z0-9.:[\]-]{1,255}\. (?:Cold-start: the host may still be warming; Spark\/SGLang can take up to 15m\. )?This is an endpoint timeout, not a bad TASK\. (?:Retry: (?:roster run --issue [1-9]\d*(?: --auto-model)?|roster run --seat coder --runtime builtin|roster doctor --warm)|Retry the same request\.)|implementation (?:model|deterministic-readme)|http chat\.completions (?:start|ok status=2\d\d|error(?: status=[1-5]\d\d)? class=(?:authentication|network|timeout|http|response|abort))|tool (?:read_file|write_file|list_dir|run_test|search_text)(?: path="(?:[^"\\]|\\.)*")?|wrote (?:RECIPE\.yml|TASK\.md|PLAN\.md|ESTIMATE\.md|RESULT\.md|REVIEW\.md)|error class=(?:Error|TypeError|RangeError|AbortError|RunLogError)|elapsed_ms=\d+ mode=(?:stub|llm))$/;
    const validMetadata = (value) => metadata.test(value) ||
      /^completion finish_reason=(?:null|"stop"|"tool_calls")$/.test(value);
    if (!parsed || !validMetadata(parsed[2]) || /[\x00-\x1f\x7f]/.test(lastLine) ||
        Buffer.byteLength(lastLine) > maximumLineBytes) {
      throw new RunLogError('Last live run log line has invalid metadata');
    }
    const valid = lines.filter((line) => {
      const match = /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z (?:start seat|seat) (planner|coder|reviewer) (.+)$/.exec(line);
      return match && validMetadata(match[2]) && !/[\x00-\x1f\x7f]/.test(line) && Buffer.byteLength(line) <= maximumLineBytes;
    });
    const lastErrorClass = valid.findLast((line) => /\bclass=/.test(line))?.match(/\bclass=([A-Za-z]+)/)?.[1] ?? null;
    return { path: file, session, lastSeat: parsed[1], lastLine: redactSecrets(lastLine, { env, apiKeyEnv }),
      lastErrorClass, lines: valid.slice(-limit).map((line) => redactSecrets(line, { env, apiKeyEnv })) };
  } finally {
    await handle.close();
  }
}

export async function readIssueLogs({ repoRoot, issue, env = process.env, apiKeyEnv = 'ROSTER_API_KEY', limit = 50 }) {
  if (!Number.isSafeInteger(Number(issue)) || Number(issue) < 1) throw new TypeError('Issue log requires a positive issue number');
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 200) throw new TypeError('Run log tail limit must be 1-200');
  const directory = path.join(repoRoot, '.roster', 'runs');
  await ensureLocalPath(directory, repoRoot);
  const entries = await fs.readdir(directory, { withFileTypes: true }).catch((error) => {
    if (error.code === 'ENOENT') return [];
    throw error;
  });
  const pattern = new RegExp(`^roster-${Number(issue)}-[A-Za-z0-9._-]+\\.log$`);
  const logs = [];
  for (const entry of entries.filter((entry) => pattern.test(entry.name)).sort((a, b) => a.name.localeCompare(b.name))) {
    if (!entry.isFile()) throw new RunLogError('Issue log must be a regular file');
    const log = await readLastRunLog({ repoRoot, session: entry.name.slice(0, -4), env, apiKeyEnv, limit });
    if (log) logs.push(log);
  }
  return logs;
}
