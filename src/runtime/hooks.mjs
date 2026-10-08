import { execFile, spawn } from 'node:child_process';
import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { parseCatalogYaml } from '../lib/catalog-yaml.mjs';
import { ensureLocalPath } from '../lib/paths.mjs';
import { redactEvidence, snapshotWorktree } from './excellence.mjs';
import { RunCancelledError, throwIfCancelled } from './cancel.mjs';

const executeFile = promisify(execFile);
async function execute(program, args, options) {
  const pending = executeFile(program, args, options);
  const closed = new Promise((resolve) => pending.child.once('close', resolve));
  try {
    const result = await pending;
    await closed;
    return result;
  } catch (error) {
    await closed;
    throw error;
  }
}

export const hookEvents = Object.freeze(['pre-plan', 'post-coder', 'pre-publish']);
export const hookScriptPattern = /^\.roster\/hooks\/[A-Za-z0-9_-]+\.[cm]?js$/;
const manifest = '.roster/hooks.yml';
const outputLimit = 4096;

export function parseHooks(source) {
  const entries = parseCatalogYaml(source, { root: 'hooks', fields: ['event', 'script', 'timeout_ms'] });
  if (entries.length > 16) throw new TypeError('At most 16 lifecycle hooks may be configured');
  return entries.map((entry) => {
    if (!hookEvents.includes(entry.event)) throw new TypeError('Hook event must be pre-plan, post-coder, or pre-publish');
    if (typeof entry.script !== 'string' || !hookScriptPattern.test(entry.script)) {
      throw new TypeError('Hook script must be a Node script directly under .roster/hooks/');
    }
    const timeout = entry.timeout_ms ?? 10000;
    if (!Number.isSafeInteger(timeout) || timeout < 1 || timeout > 60000) {
      throw new TypeError('Hook timeout_ms must be an integer from 1 to 60000');
    }
    return { ...entry, timeout_ms: timeout };
  });
}

// An allow-list, not a secret-name deny-list: arbitrary endpoint keys and App/git variables never pass through.
export function hookEnvironment(env = process.env) {
  const allowed = /^(?:path|pathext|systemroot|windir|comspec|temp|tmp|lang|lc_all|tz)$/i;
  return Object.fromEntries(Object.entries(env).filter(([name, value]) => allowed.test(name) && typeof value === 'string'));
}

async function readHookFile(worktree, file, optional = false) {
  const target = path.join(worktree, file);
  if (optional) {
    const parent = await fs.lstat(path.dirname(target)).catch((error) => {
      if (error.code === 'ENOENT') return null;
      throw error;
    });
    // Legacy standalone worktrees may have a regular state marker, which cannot contain a hook manifest.
    if (parent?.isFile()) return null;
  }
  await ensureLocalPath(target, worktree);
  let handle;
  try {
    handle = await fs.open(target, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  } catch (error) {
    if (optional && error.code === 'ENOENT') return null;
    throw error;
  }
  try {
    const entry = await handle.stat();
    if (!entry.isFile() || entry.nlink !== 1 || entry.size > 65536) {
      throw new Error('Hook files must be regular single-link files of at most 64 KiB');
    }
    return await handle.readFile('utf8');
  } finally {
    await handle.close();
  }
}

async function trustedHooks(worktree, env, signal) {
  const source = await readHookFile(worktree, manifest, true);
  if (source === null) return [];
  const hooks = parseHooks(source);
  if (!hooks.length) return [];
  for (const [file, current] of [[manifest, source], ...await Promise.all(
    [...new Set(hooks.map(({ script }) => script))].map(async (script) => [script, await readHookFile(worktree, script)]))]) {
    let committed;
    try {
      ({ stdout: committed } = await execute('git', ['--no-pager', 'show', `HEAD:${file}`], {
        cwd: worktree, env: hookEnvironment(env), encoding: 'utf8', maxBuffer: 65536, timeout: 10000, signal,
      }));
    } catch (error) {
      throwIfCancelled(signal);
      throw new Error(`Lifecycle hook must be human-reviewed and committed at HEAD: ${file}`, { cause: error });
    }
    if (current.replace(/\r\n/g, '\n') !== committed.replace(/\r\n/g, '\n')) {
      throw new Error(`Lifecycle hook differs from committed HEAD: ${file}`);
    }
  }
  return hooks;
}

async function gitState(worktree, env, signal) {
  const options = { cwd: worktree, env: hookEnvironment(env), encoding: 'utf8',
    timeout: 10000, maxBuffer: 4 * 1024 * 1024, signal };
  try {
    const results = await Promise.allSettled([
      execute('git', ['rev-parse', '--verify', 'HEAD'], options),
      execute('git', ['--no-pager', 'diff', '--cached', '--raw', '--no-ext-diff', '--no-textconv', '-z'], options),
    ]);
    const failure = results.find(({ status }) => status === 'rejected');
    if (failure) throw failure.reason;
    return results.map(({ value }) => value.stdout).join('');
  } catch (error) {
    throwIfCancelled(signal);
    throw new Error('Lifecycle hook Git state inspection failed', { cause: error });
  }
}

function runScript(hook, { worktree, env, signal }) {
  throwIfCancelled(signal);
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [path.join(worktree, hook.script)], {
      cwd: worktree, env: hookEnvironment(env), shell: false, windowsHide: true,
      // A Unix process group permits killing descendants without detaching our wait or stdio.
      detached: process.platform !== 'win32', stdio: ['ignore', 'pipe', 'pipe'],
    });
    const chunks = [];
    let bytes = 0;
    let stopped;
    let timer;
    let killError;
    let termination;
    let finished = false;
    const stop = (reason) => {
      if (stopped) return;
      stopped = reason;
      if (!child.pid) return;
      if (process.platform === 'win32') {
        // Kill only this owned hook tree, never a process name shared with other users.
        termination = execute(path.join(process.env.SystemRoot, 'System32', 'taskkill.exe'),
          ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true, timeout: 5000, env: hookEnvironment(env) })
          .catch((error) => { killError = error; child.kill('SIGKILL'); });
      } else {
        try {
          process.kill(-child.pid, 'SIGKILL');
        } catch (error) {
          if (error.code !== 'ESRCH') { killError = error; child.kill('SIGKILL'); }
        }
      }
      // A child that exited while descendants retain its pipes must not keep the harness waiting forever.
      Promise.resolve(termination).then(() => {
        child.stdout.destroy();
        child.stderr.destroy();
      });
    };
    const abort = () => stop('cancelled');
    const collect = (chunk) => {
      bytes += chunk.length;
      if (bytes > outputLimit) stop('output-limit');
      else chunks.push(chunk);
    };
    child.stdout.on('data', collect);
    child.stderr.on('data', collect);
    const finish = async (error, code, exitSignal) => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      signal?.removeEventListener('abort', abort);
      await termination;
      if (stopped === 'cancelled' || signal?.aborted) return reject(new RunCancelledError());
      if (error) return reject(new Error('Lifecycle hook could not start', { cause: error }));
      if (killError) return reject(new Error('Lifecycle hook process-tree termination failed', { cause: killError }));
      resolve({ script: hook.script, exit_code: code, signal: exitSignal,
        status: stopped ?? (code === 0 ? 'pass' : 'fail'),
        output: stopped === 'output-limit' ? '' : Buffer.concat(chunks).toString('utf8') });
    };
    child.once('error', (error) => finish(error));
    child.once('close', (code, exitSignal) => finish(null, code, exitSignal));
    timer = setTimeout(() => stop('timeout'), hook.timeout_ms);
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted) abort();
  });
}

export async function runLifecycleHooks(event, {
  worktree, env = process.env, apiKeyEnv, signal, memoryPath, onEvent,
} = {}) {
  if (!hookEvents.includes(event)) throw new TypeError('Unknown lifecycle hook event');
  throwIfCancelled(signal);
  const hooks = (await trustedHooks(worktree, env, signal)).filter((hook) => hook.event === event);
  throwIfCancelled(signal);
  const entries = [];
  const reasons = [];
  for (const hook of hooks) {
    throwIfCancelled(signal);
    const before = await snapshotWorktree(worktree, { memoryPath });
    const beforeGit = await gitState(worktree, env, signal);
    const started = performance.now();
    const entry = await runScript(hook, { worktree, env, signal });
    entry.duration_ms = Math.max(0, Math.round(performance.now() - started));
    const after = await snapshotWorktree(worktree, { memoryPath });
    const changedGit = beforeGit !== await gitState(worktree, env, signal);
    const changed = [...new Set([...before.keys(), ...after.keys()])].filter((file) => before.get(file) !== after.get(file));
    if (changed.length || changedGit) reasons.push(`Lifecycle hook: ${event} ${hook.script} changed worktree files or Git state; hooks must be read-only.`);
    const safe = Buffer.from(redactEvidence(entry.output, { env, apiKeyEnv }), 'utf8');
    let start = Math.max(0, safe.length - outputLimit);
    while (start < safe.length && (safe[start] & 0xc0) === 0x80) start += 1;
    entry.output = safe.subarray(start).toString('utf8');
    entries.push(entry);
    if (entry.status !== 'pass') {
      const failure = entry.status === 'timeout' ? `timed out after ${hook.timeout_ms} ms` :
        entry.status === 'output-limit' ? 'exceeded the 4 KiB output limit' :
          `failed (exit ${entry.exit_code ?? entry.signal ?? 'unknown'})`;
      reasons.push(`Lifecycle hook: ${event} ${hook.script} ${failure}${entry.output.trim() ? `: ${entry.output.trim()}` : ''}`);
    }
    if (changed.length || changedGit) entry.status = 'mutation';
    await onEvent?.({ type: 'lifecycle-hook', event, script: hook.script,
      status: changed.length || changedGit ? 'fail' : entry.status, ms: entry.duration_ms });
    if (reasons.length) break;
  }
  return { pass: reasons.length === 0, reasons, entries };
}

export async function requireLifecycleHooks(event, options) {
  const result = await runLifecycleHooks(event, options);
  if (!result.pass) throw new Error(result.reasons.join('\n'));
  return result;
}
