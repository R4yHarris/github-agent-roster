import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { ensureLocalPath } from '../lib/paths.mjs';
import { isForbiddenRead, isForbiddenWrite } from '../runtime/tools.mjs';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));
const coderCapabilities = Object.freeze(['commit_branch', 'open_pr']);
const coderDeny = Object.freeze({
  capabilities: Object.freeze(['merge', 'push_protected', 'deploy']),
  read: isForbiddenRead,
  write: isForbiddenWrite,
  scope: 'TASK.md',
});
const reviewerCapabilities = Object.freeze(['comment']);
const reviewerDeny = Object.freeze({
  capabilities: Object.freeze(['commit_branch', 'open_pr', 'merge', 'push_protected', 'deploy']),
  read: isForbiddenRead,
  write: () => true,
  scope: 'REVIEW.md',
});

export async function loadPrincipal({ repoRoot = rosterRoot, id = 'coder' } = {}) {
  if (!['coder', 'reviewer'].includes(id)) {
    throw new TypeError('The builtin SWE principal must be coder or reviewer');
  }
  const file = path.join(repoRoot, 'principals', `${id}.md`);
  await ensureLocalPath(file, repoRoot);
  let content;
  try {
    const entry = await fs.lstat(file);
    if (!entry.isFile() || entry.size > 65_536) {
      throw new Error(`${id} principal must be a regular Markdown file of at most 64 KiB`);
    }
    content = await fs.readFile(file, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') throw new Error(`Missing ${id} principal: ${file}`, { cause: error });
    throw error;
  }
  if (!content.trim()) throw new Error(`${id} principal must contain conduct instructions`);
  return Object.freeze({
    id, role: id, content,
    capabilities: id === 'reviewer' ? reviewerCapabilities : coderCapabilities,
    deny: id === 'reviewer' ? reviewerDeny : coderDeny,
  });
}
