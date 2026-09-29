import { promises as fs } from 'node:fs';
import path from 'node:path';
import { readMemory } from './memory.mjs';

async function requiredFile(file) {
  const stat = await fs.lstat(file);
  if (!stat.isFile() || stat.isSymbolicLink()) {
    throw new Error(`${path.basename(file)} must be a regular file, not a symlink`);
  }
  return await fs.readFile(file, 'utf8');
}

export async function loadContext({ worktree, memoryPath, repoRoot }) {
  const [agents, task, memory] = await Promise.all([
    requiredFile(path.join(worktree, 'AGENTS.md')),
    requiredFile(path.join(worktree, 'TASK.md')),
    readMemory({ file: memoryPath, repoRoot, limit: 20 }),
  ]);
  return { agents, task, memory };
}
