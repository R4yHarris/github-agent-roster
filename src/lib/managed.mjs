import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';

const execute = promisify(execFile);

export async function ensureManagedIgnored(worktree, pattern, env = process.env) {
  if (!['.roster/checkpoints/', '.roster/map.md'].includes(pattern)) throw new TypeError('Unsupported managed ignore pattern');
  const marker = await fs.lstat(path.join(worktree, '.git')).catch((error) => {
    if (error.code === 'ENOENT') return null;
    throw error;
  });
  if (!marker) return;
  const { stdout } = await execute('git', ['rev-parse', '--git-path', 'info/exclude'], {
    cwd: worktree, env, encoding: 'utf8', timeout: 30000,
  });
  const file = path.resolve(worktree, stdout.trim());
  const entry = await fs.lstat(file).catch((error) => { if (error.code === 'ENOENT') return null; throw error; });
  if (entry && (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1)) {
    throw new Error('Git exclude must be a regular single-link file before storing managed artifacts');
  }
  const source = entry ? await fs.readFile(file, 'utf8') : '';
  if (!source.split(/\r?\n/).includes(pattern)) {
    await fs.mkdir(path.dirname(file), { recursive: true });
    await fs.appendFile(file, `${source.endsWith('\n') || !source ? '' : '\n'}${pattern}\n`);
  }
}
