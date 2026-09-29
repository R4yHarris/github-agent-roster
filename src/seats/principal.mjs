import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { ensureLocalPath } from '../lib/paths.mjs';
import { isForbiddenRead, isForbiddenWrite } from '../runtime/tools.mjs';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));
const capabilities = Object.freeze(['commit_branch', 'open_pr']);
const deny = Object.freeze({
  capabilities: Object.freeze(['merge', 'push_protected', 'deploy']),
  read: isForbiddenRead,
  write: isForbiddenWrite,
  scope: 'TASK.md',
});

export async function loadPrincipal({ repoRoot = rosterRoot, id = 'coder' } = {}) {
  if (id !== 'coder') throw new TypeError('The builtin SWE principal must be coder');
  const file = path.join(repoRoot, 'principals', `${id}.md`);
  await ensureLocalPath(file, repoRoot);
  let content;
  try {
    const entry = await fs.lstat(file);
    if (!entry.isFile() || entry.size > 65_536) {
      throw new Error('Coder principal must be a regular Markdown file of at most 64 KiB');
    }
    content = await fs.readFile(file, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') throw new Error(`Missing coder principal: ${file}`, { cause: error });
    throw error;
  }
  if (!content.trim()) throw new Error('Coder principal must contain conduct instructions');
  return Object.freeze({ id, role: 'coder', content, capabilities, deny });
}
