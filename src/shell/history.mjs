import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify, TextDecoder } from 'node:util';
import { ensureLocalPath } from '../lib/paths.mjs';
import { redactSecrets } from '../runtime/memory.mjs';

const execute = promisify(execFile);
const maxLines = 200;

export function safeHistoryLine(line, { env = process.env, secret = false } = {}) {
  return !secret && typeof line === 'string' && line.trim() &&
    Buffer.byteLength(line) <= 16384 && !/[\x00-\x1f\x7f]/.test(line) &&
    !/^\/vault(?:\s|$)/i.test(line.trim()) && !/\.pem(?:\b|$)/i.test(line) &&
    !/\b(?:password|passwd|token|private[_ -]?key|authorization|bearer)\b/i.test(line) &&
    redactSecrets(line, { env }) === line;
}

export function createHistory({ repoRoot, env = process.env } = {}) {
  const file = path.join(repoRoot, '.roster', 'history');
  let lines = [];
  let pending = Promise.resolve();
  let loaded = false;

  async function load() {
    await ensureLocalPath(file, repoRoot);
    let handle;
    try {
      handle = await fs.open(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
    if (handle) {
      try {
        const entry = await handle.stat();
        if (!entry.isFile() || entry.nlink !== 1 || entry.size > 4 * 1024 * 1024) {
          throw new Error('Shell history must be a regular, single-link file of at most 4 MiB');
        }
        const content = new TextDecoder('utf-8', { fatal: true }).decode(await handle.readFile());
        lines = content.split(/\r?\n/).filter((line) => safeHistoryLine(line, { env })).slice(-maxLines);
        if (process.platform !== 'win32') await fs.chmod(file, 0o600);
      } finally {
        await handle.close();
      }
    }
    loaded = true;
    return [...lines];
  }

  async function record(line, { secret = false } = {}) {
    if (!loaded) throw new Error('Load shell history before recording commands');
    if (!safeHistoryLine(line, { env, secret })) return;
    lines = [...lines.filter((entry) => entry !== line), line].slice(-maxLines);
    const snapshot = `${lines.join('\n')}\n`;
    const write = pending.then(async () => {
      await ensureLocalPath(file, repoRoot);
      const marker = await fs.lstat(path.join(repoRoot, '.git')).catch((error) => {
        if (error.code === 'ENOENT') return null;
        throw error;
      });
      if (marker) {
        try {
          await execute('git', ['check-ignore', '--quiet', '--', '.roster/history'], {
            cwd: repoRoot, encoding: 'utf8', timeout: 10_000,
          });
        } catch (error) {
          if (error.code === 1) throw new Error('Shell history must be untracked and gitignored; ignore .roster/history');
          throw error;
        }
      }
      await fs.mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
      await ensureLocalPath(file, repoRoot);
      const current = await fs.lstat(file).catch((error) => {
        if (error.code === 'ENOENT') return null;
        throw error;
      });
      if (current && (!current.isFile() || current.isSymbolicLink() || current.nlink !== 1)) {
        throw new Error('Shell history must remain a regular, single-link file');
      }
      const temporary = `${file}.${randomBytes(8).toString('hex')}`;
      try {
        await fs.writeFile(temporary, snapshot, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
        await fs.rename(temporary, file);
      } finally {
        await fs.unlink(temporary).catch((error) => {
          if (error.code !== 'ENOENT') throw error;
        });
      }
    });
    pending = write;
    await write;
  }

  return { load, record, flush: () => pending, get lines() { return [...lines]; }, path: file };
}
