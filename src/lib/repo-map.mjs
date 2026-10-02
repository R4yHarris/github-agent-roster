import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { ensureLocalPath } from './paths.mjs';
import { ensureManagedIgnored } from './managed.mjs';
import { readPlannerTask } from '../seats/planner.mjs';
import { taskFilesAllowed } from '../planner/task.mjs';
import { isForbiddenRead, isManagedFile } from '../runtime/tools.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';

export async function writeRepoMap({ worktree, env = process.env, apiKeyEnv }) {
  const task = await readPlannerTask(worktree);
  if (task === null) throw new Error('A current TASK.md is required before /map.');
  const named = taskFilesAllowed(task);
  const rows = ['# Repository map (filenames only)', '', '## TASK paths', ...named.map((file) => `- ${file}`),
    '', '## Top two directory levels'];
  let truncated = false;
  const visible = (file) => !isForbiddenRead(file) && !isManagedFile(file) &&
    !['.roster', '.worktrees', 'node_modules'].includes(file.split('/')[0]) &&
    redactEvidence(file, { env, apiKeyEnv }) === file;
  async function visit(directory = '', depth = 0) {
    for (const entry of (await fs.readdir(path.join(worktree, directory), { withFileTypes: true }))
      .sort((left, right) => left.name.localeCompare(right.name))) {
      const file = path.posix.join(directory, entry.name);
      if (!visible(file) || entry.isSymbolicLink() || !entry.isFile() && !entry.isDirectory()) continue;
      const line = `- ${file}${entry.isDirectory() ? '/' : ''}`;
      if (rows.length === 79 || Buffer.byteLength(`${rows.join('\n')}\n${line}\n`) > 32000) {
        truncated = true; return;
      }
      rows.push(line);
      if (entry.isDirectory() && depth < 1) await visit(file, depth + 1);
      if (truncated) return;
    }
  }
  await visit();
  if (truncated) rows.push('- (additional filenames omitted at the 80-line cap)');
  const content = `${rows.join('\n')}\n`;
  const file = path.join(worktree, '.roster', 'map.md');
  await ensureLocalPath(file, worktree);
  await ensureManagedIgnored(worktree, '.roster/map.md', env);
  await fs.mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
  const entry = await fs.lstat(file).catch((error) => { if (error.code === 'ENOENT') return null; throw error; });
  if (entry && (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1)) throw new Error('Repo map must be a regular single-link file');
  const handle = await fs.open(file, constants.O_WRONLY | constants.O_CREAT | constants.O_TRUNC | (constants.O_NOFOLLOW ?? 0), 0o600);
  try { await handle.writeFile(content, 'utf8'); }
  finally { await handle.close(); }
  return { path: file, lines: rows.length };
}

export async function readRepoMap(worktree, { env = process.env, apiKeyEnv } = {}) {
  const file = path.join(worktree, '.roster', 'map.md');
  await ensureLocalPath(file, worktree);
  const entry = await fs.lstat(file).catch((error) => { if (error.code === 'ENOENT') return null; throw error; });
  if (!entry) return null;
  if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1 || entry.size > 32768) {
    throw new Error('Repo map must be a bounded regular single-link file');
  }
  const content = await fs.readFile(file, 'utf8');
  if (content.trimEnd().split('\n').length > 80 || !content.startsWith('# Repository map (filenames only)\n') ||
      redactEvidence(content, { env, apiKeyEnv }) !== content) throw new Error('Repo map metadata is invalid');
  if (content.trimEnd().split('\n').some((line) => line &&
      !['# Repository map (filenames only)', '## TASK paths', '## Top two directory levels'].includes(line) &&
      !line.startsWith('- '))) throw new Error('Repo map may contain only filename metadata');
  return content;
}
