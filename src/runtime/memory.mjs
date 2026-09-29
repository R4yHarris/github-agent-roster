import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { ensureLocalPath } from '../lib/paths.mjs';

export function seatMemoryPath({ repoRoot, memoryPath, seat }) {
  const coder = path.join(repoRoot, memoryPath);
  if (seat === 'coder') return coder;
  if (seat === 'planner') return path.join(path.dirname(coder), 'planner.jsonl');
  throw new TypeError('Memory seat must be planner or coder');
}

export function redactSecrets(text, { env = process.env, apiKeyEnv = 'ROSTER_API_KEY' } = {}) {
  if (typeof text !== 'string') throw new TypeError('Redaction requires text');
  const secrets = Object.entries(env)
    .filter(([name, value]) => typeof value === 'string' && value &&
      (name === apiKeyEnv || /TOKEN|PASSWORD|SECRET|PRIVATE_KEY|API_KEY/i.test(name)))
    .map(([, value]) => value).sort((left, right) => right.length - left.length);
  for (const secret of secrets) text = text.split(secret).join('[redacted]');
  return text.replace(/-----BEGIN [^-\r\n]*PRIVATE KEY-----[\s\S]*/g, '[redacted]')
    .replace(/\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{20,})\b/g, '[redacted]')
    .replace(/(\bhttps?:\/\/)[^/\s]+@/gi, '$1[redacted]@')
    .replace(/\b(Bearer\s+|[A-Za-z0-9_-]*(?:api[_-]?key|password|secret|token)[A-Za-z0-9_-]*["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,;]+)/gi,
      '$1[redacted]');
}

function safeRecord(record, options = {}) {
  if (!record || typeof record !== 'object' || Array.isArray(record) ||
      ![Object.prototype, null].includes(Object.getPrototypeOf(record))) {
    throw new TypeError('Memory record must be a plain JSON object');
  }
  const safe = {};
  for (const [key, value] of Object.entries(record)) {
    if (!/^[a-z][a-z0-9_]*$/i.test(key) ||
        /(?:^|_)(?:content|body|messages|password|secret|token|key)(?:_|$)/i.test(key)) {
      throw new TypeError('Memory accepts notebook metadata, not credentials or file/message bodies');
    }
    if (value === null || typeof value === 'boolean' ||
        (typeof value === 'number' && Number.isFinite(value))) {
      safe[key] = value;
      continue;
    }
    if (typeof value !== 'string') throw new TypeError('Memory values must be compact scalar summaries');
    const text = redactSecrets(value, options);
    const first = text.split(/\r?\n/)[0].trim();
    safe[key] = first.length > 480 || first !== text.trim()
      ? `${first.slice(0, 480)} [details omitted]` : first;
  }
  return safe;
}

export function coderMemoryRecord({ task, session, mode, changedFiles = [], tests, error, time = new Date().toISOString() }) {
  const match = /^issue-([1-9]\d*)$/.exec(task);
  const changed = changedFiles.length
    ? `Updated ${changedFiles.length} task-scoped file(s): ${changedFiles.slice(0, 4).join(', ')}` : 'No code changes.';
  const testResult = tests ? `node --test exited ${tests.exit_code}` : 'Not run.';
  return {
    time, issue: match ? Number(match[1]) : null, task, session,
    changed, tests: testResult,
    next_gap: error?.message ?? (mode === 'stub'
      ? 'Configure an LLM endpoint to implement and test this task.' : 'None reported.'),
    status: error ? 'failed' : mode,
    summary: `${changed} ${testResult}`,
  };
}

export async function readMemory({ file, repoRoot, limit = 20, env, apiKeyEnv }) {
  if (!Number.isSafeInteger(limit) || limit < 0) {
    throw new TypeError('Memory tail limit must be a nonnegative safe integer');
  }
  await ensureLocalPath(file, repoRoot);
  if (limit === 0) return [];
  let handle;
  try {
    handle = await fs.open(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  } catch (error) {
    if (error.code === 'ENOENT') return [];
    throw error;
  }
  let contents;
  let position;
  try {
    const stat = await handle.stat();
    if (!stat.isFile()) throw new Error('Memory must be a regular JSONL file');
    position = stat.size;
    const chunks = [];
    let newlines = 0;
    while (position > 0 && newlines <= limit) {
      const size = Math.min(position, 4096);
      position -= size;
      const chunk = Buffer.alloc(size);
      const { bytesRead } = await handle.read(chunk, 0, size, position);
      if (bytesRead !== size) throw new Error('Memory changed while reading its tail');
      for (const byte of chunk) if (byte === 10) newlines += 1;
      chunks.unshift(chunk);
    }
    contents = Buffer.concat(chunks).toString('utf8');
    if (stat.size && !contents.endsWith('\n')) {
      throw new Error('Memory ends with an incomplete JSONL line');
    }
  } finally {
    await handle.close();
  }
  const lines = contents.split(/\r?\n/);
  if (lines.at(-1) === '') lines.pop();
  if (position > 0) lines.shift();
  const last = lines.slice(-limit);
  return last.map((line, index) => {
    let record;
    try {
      record = JSON.parse(line);
    } catch {
      throw new Error(position === 0
        ? `Invalid memory JSONL line ${lines.length - last.length + index + 1}`
        : `Invalid memory JSONL tail entry ${index + 1}`);
    }
    return JSON.stringify(safeRecord(record, { env, apiKeyEnv }));
  });
}

export async function appendMemory({ file, repoRoot, record, env, apiKeyEnv }) {
  await ensureLocalPath(file, repoRoot);
  const content = `${JSON.stringify(safeRecord(record, { env, apiKeyEnv }))}\n`;
  if (Buffer.byteLength(content, 'utf8') > 4096) throw new TypeError('Memory record exceeds 4096 bytes');
  await fs.mkdir(path.dirname(file), { recursive: true });
  await ensureLocalPath(file, repoRoot);
  const handle = await fs.open(file, constants.O_RDWR | constants.O_APPEND | constants.O_CREAT |
    (constants.O_NOFOLLOW ?? 0), 0o600);
  try {
    const stat = await handle.stat();
    if (!stat.isFile()) throw new Error('Memory must be a regular JSONL file');
    if (stat.size) {
      const last = Buffer.alloc(1);
      await handle.read(last, 0, 1, stat.size - 1);
      if (last[0] !== 10) throw new Error('Memory ends with an incomplete JSONL line; refusing to rewrite it');
    }
    await handle.writeFile(content, 'utf8');
  } finally {
    await handle.close();
  }
}
