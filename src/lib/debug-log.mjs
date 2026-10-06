import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { promisify } from 'node:util';
import { ensureLocalPath } from './paths.mjs';
import { redactSecrets } from '../runtime/memory.mjs';

const execute = promisify(execFile);
const seats = ['planner', 'coder', 'reviewer'];
const tools = ['read_file', 'write_file', 'edit_file', 'delete_file', 'glob_files', 'list_dir', 'run_test', 'run_command', 'search_text', 'web_search', 'web_fetch'];
const pathClasses = ['root', 'outside', 'secret', 'git', 'policy', 'workflow', 'vendor', 'managed', 'tests', 'source', 'docs', 'other'];
const phases = ['seat-start', 'seat-end', 'seat-error', 'model', 'http-start', 'http-ok', 'http-error',
  'waiting', 'timeout', 'timeout-retry', 'stall', 'served-model', 'tool-start', 'tool-ok', 'tool-error', 'tool-denied', 'tool-refused', 'finish-reason',
  'finish-retry', 'completion', 'steering', 'test-repair', 'contracts-uninitialized', 'wrote', 'implementation',
  'usage'];
const fields = ['time', 'issue', 'seat', 'phase', 'tool_name', 'path_class', 'finish_reason',
  'test_name', 'exit_code', 'repair', 'elapsed_ms'];
const finishReasons = ['stop', 'tool_calls', 'length', 'content_filter', 'function_call', 'eos_token',
  'error', 'cancelled', 'abort', 'redacted', 'unsupported'];

export class DebugLogError extends Error {
  code = 'ROSTER_RUN_LOG';
}

export function debugPathClass(value) {
  if (typeof value !== 'string') return null;
  const file = value.replaceAll('\\', '/').toLowerCase();
  const parts = file.split('/');
  if (parts.some((part) => part === '.env' || part.startsWith('.env.') || part.endsWith('.pem')) ||
      file.includes('.roster/vault')) return 'secret';
  if (path.isAbsolute(value) || path.win32.isAbsolute(value) || parts.includes('..')) return 'outside';
  if (parts.includes('.git')) return 'git';
  if (parts.includes('agent-policy.yml')) return 'policy';
  if (file.includes('.github/workflows')) return 'workflow';
  if (parts.includes('vendor')) return 'vendor';
  if (file.startsWith('.roster/') || ['assignment.md', 'task.md', 'recipe.yml', 'plan.md', 'context.md',
    'research.md', 'result.md', 'review.md', 'estimate.md'].includes(file)) return 'managed';
  if (parts.includes('tests') || parts.includes('test') || parts.includes('fixtures') || /[._-]test\.[cm]?js$/.test(file)) return 'tests';
  if (parts[0] === 'src') return 'source';
  if (parts[0] === 'docs' || file.endsWith('.md')) return 'docs';
  return ['', '.', './'].includes(file) ? 'root' : 'other';
}

function validRow(row) {
  return row && typeof row === 'object' && !Array.isArray(row) &&
    Object.keys(row).length === fields.length && fields.every((field) => Object.hasOwn(row, field)) &&
    typeof row.time === 'string' && /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(row.time) &&
    (row.issue === null || Number.isSafeInteger(row.issue) && row.issue > 0) &&
    seats.includes(row.seat) && phases.includes(row.phase) &&
    (row.tool_name === null || tools.includes(row.tool_name)) &&
    (row.path_class === null || pathClasses.includes(row.path_class)) &&
    (row.finish_reason === null || finishReasons.includes(row.finish_reason)) &&
    [null, 'node --test'].includes(row.test_name) &&
    (row.exit_code === null || Number.isSafeInteger(row.exit_code) && row.exit_code >= 0) &&
    (row.repair === null || typeof row.repair === 'object' && Object.keys(row.repair).length === 2 &&
      Number.isInteger(row.repair.n) && row.repair.n >= 1 && row.repair.n <= row.repair.of &&
      [1, 2, 4].includes(row.repair.of)) &&
    Number.isSafeInteger(row.elapsed_ms) && row.elapsed_ms >= 0;
}

export function createDebugLog({
  env = process.env, enabled = env.ROSTER_DEBUG === '1',
  session = `roster-${randomBytes(8).toString('hex')}`,
  now = () => new Date(), clock = () => performance.now(),
} = {}) {
  if (typeof enabled !== 'boolean' || typeof now !== 'function' || typeof clock !== 'function' ||
      typeof session !== 'string' || !/^[A-Za-z0-9._-]{1,64}$/.test(session)) {
    throw new TypeError('Invalid process debug log settings');
  }
  const sessionIsSafe = () => !Object.values(env).includes(session) && redactSecrets(session, { env }) === session;
  if (enabled && !sessionIsSafe()) throw new TypeError('Debug session must not contain environment values');
  const started = clock();
  let revision = 0;
  let latestPath = null;
  let latestRoot = null;
  let pending = Promise.resolve();
  const checkedRoots = new Set();
  const safeReason = (value) => {
    if (value === undefined) return null;
    if (typeof value !== 'string' || !/^[A-Za-z0-9_.-]{1,64}$/.test(value)) return 'unsupported';
    if (Object.values(env).some((entry) => typeof entry === 'string' && entry && value.includes(entry)) ||
        redactSecrets(value, { env }) !== value) return 'redacted';
    return finishReasons.includes(value) ? value : 'unsupported';
  };

  async function record({ repoRoot, issue = null, seat, event }) {
    if (!enabled) return;
    let phase = event.type;
    if (event.type === 'http') phase = `http-${event.phase}`;
    if (event.type === 'tool') phase = 'tool-start';
    if (event.type === 'tool-result') phase = `tool-${event.status}`;
    if (event.type === 'finish-reason' && event.retry) phase = 'finish-retry';
    const timestamp = now();
    if (!(timestamp instanceof Date) || !Number.isFinite(timestamp.getTime())) {
      throw new DebugLogError('Invalid debug event time');
    }
    const row = {
      time: timestamp.toISOString(), issue, seat, phase,
      tool_name: ['tool', 'tool-result', 'tool-refused'].includes(event.type) ? event.name : null,
      path_class: ['tool', 'tool-result', 'wrote'].includes(event.type) ? debugPathClass(event.path)
        : event.type === 'tool-refused' ? 'outside' : null,
      finish_reason: ['finish-reason', 'completion'].includes(event.type) ? safeReason(event.reason ?? undefined) : null,
      test_name: ['tool', 'tool-result'].includes(event.type) && event.name === 'run_test' ? 'node --test' : null,
      exit_code: event.type === 'tool-result' ? event.exit_code ?? null : null,
      repair: event.type === 'test-repair' ? { n: event.attempt, of: event.budget } : null,
      elapsed_ms: Math.max(0, Math.round(clock() - started)),
    };
    if (!validRow(row)) return;
    const generation = revision;
    const write = pending.then(async () => {
      if (!enabled || generation !== revision) return;
      const file = path.join(repoRoot, '.roster', 'logs', `debug-${session}.jsonl`);
      await ensureLocalPath(file, repoRoot);
      if (!checkedRoots.has(repoRoot)) {
        const marker = await fs.lstat(path.join(repoRoot, '.git')).catch((error) => {
          if (error.code === 'ENOENT') return null;
          throw error;
        });
        if (marker) {
          try {
            await execute('git', ['check-ignore', '--quiet', '--', path.relative(repoRoot, file).split(path.sep).join('/')], {
              cwd: repoRoot, encoding: 'utf8', timeout: 10_000,
            });
          } catch (error) {
            if (error.code === 1) throw new DebugLogError('Debug logs must be untracked and gitignored; ignore .roster/logs/ before enabling');
            throw error;
          }
        }
        checkedRoots.add(repoRoot);
      }
      if (!enabled || generation !== revision) return;
      await fs.mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
      await ensureLocalPath(file, repoRoot);
      const handle = await fs.open(file, constants.O_WRONLY | constants.O_APPEND | constants.O_CREAT |
        (constants.O_NOFOLLOW ?? 0), 0o600);
      try {
        const entry = await handle.stat();
        if (!entry.isFile() || entry.nlink !== 1) throw new DebugLogError('Debug log must be a regular, single-link file');
        if (enabled && generation === revision) await handle.writeFile(`${JSON.stringify(row)}\n`, 'utf8');
      } finally {
        await handle.close();
      }
      latestPath = file;
      latestRoot = repoRoot;
    }).catch((error) => {
      if (error instanceof DebugLogError) throw error;
      throw new DebugLogError('Could not write debug log');
    });
    pending = write;
    await write;
  }

  async function tail({ limit = 50 } = {}) {
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 200) throw new TypeError('Debug tail limit must be 1-200');
    await pending;
    if (latestPath === null) return null;
    await ensureLocalPath(latestPath, latestRoot);
    const handle = await fs.open(latestPath, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
    try {
      const entry = await handle.stat();
      if (!entry.isFile() || entry.nlink !== 1) throw new DebugLogError('Debug log must be a regular, single-link file');
      const size = Math.min(entry.size, 262_144);
      const buffer = Buffer.alloc(size);
      const { bytesRead } = await handle.read(buffer, 0, size, entry.size - size);
      const lines = buffer.subarray(0, bytesRead).toString('utf8').split('\n');
      lines.pop();
      if (entry.size > size) lines.shift();
      const selected = lines.slice(-limit);
      for (const line of selected) {
        let row;
        try { row = JSON.parse(line); }
        catch { throw new DebugLogError('Invalid debug log metadata'); }
        if (!validRow(row) || row.finish_reason !== null && safeReason(row.finish_reason) !== row.finish_reason) {
          throw new DebugLogError('Invalid debug log metadata');
        }
      }
      return { path: latestPath, lines: selected };
    } finally {
      await handle.close();
    }
  }

  return {
    record, tail,
    get path() { return latestPath; },
    get enabled() { return enabled; },
    setEnabled(value) {
      if (typeof value !== 'boolean') throw new TypeError('Debug mode must be a boolean');
      if (value && !sessionIsSafe()) throw new TypeError('Debug session must not contain environment values');
      if (enabled === value) return;
      enabled = value;
      revision += 1;
    },
  };
}
