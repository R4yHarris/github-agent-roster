import { createHash, randomBytes } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { ensureLocalPath } from './paths.mjs';

const names = ['RECIPE.yml', 'TASK.md', 'PLAN.md', 'ESTIMATE.md', 'CONTEXT.md', 'RESEARCH.md', 'RESULT.md', 'REVIEW.md'];

export async function archiveRunArtifacts(worktree, { task, git, preserve = [] }) {
  if (typeof task !== 'string' || !/^(?!-$)[A-Za-z0-9._-]{1,64}$/.test(task) || typeof git !== 'function') {
    throw new TypeError('Run archive requires an opaque task identifier and Git helper');
  }
  if (!Array.isArray(preserve) ||
      preserve.some((name) => !['RECIPE.yml', 'TASK.md', 'PLAN.md', 'ESTIMATE.md'].includes(name))) {
    throw new TypeError('Run archive may preserve only validated recipe/task/plan/estimate artifacts');
  }
  const files = [];
  for (const name of names) {
    const file = path.join(worktree, name);
    await ensureLocalPath(file, worktree);
    const entry = await fs.lstat(file).catch((error) => {
      if (error.code === 'ENOENT') return null;
      throw error;
    });
    if (!entry) continue;
    if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1) {
      throw new Error('Previous run artifacts must be regular, single-link files');
    }
    if (preserve.includes(name)) continue;
    files.push({ name, file });
  }
  if (!files.length) return null;
  if ((await git(['ls-files', '-z', '--', ...files.map(({ name }) => name)])).trim()) {
    throw new Error('Refusing to replace tracked planning/run artifacts');
  }
  const common = path.resolve(worktree, (await git(['rev-parse', '--git-common-dir'])).trim());
  const archiveTask = /^(?:issue-[1-9]\d*|local-[a-f0-9]{16})$/.test(task)
    ? task : `task-${createHash('sha256').update(task).digest('hex').slice(0, 16)}`;
  const directory = path.join(common, 'roster-artifacts', archiveTask,
    `${Date.now()}-${randomBytes(6).toString('hex')}`);
  await ensureLocalPath(directory, common);
  await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  const moved = [];
  try {
    for (const { name, file } of files) {
      await ensureLocalPath(file, worktree);
      const destination = path.join(directory, name);
      await fs.rename(file, destination);
      moved.push({ file, destination });
    }
  } catch (error) {
    for (const { file, destination } of moved.reverse()) {
      const existing = await fs.lstat(file).catch((failure) => {
        if (failure.code === 'ENOENT') return null;
        throw failure;
      });
      if (existing) {
        throw new Error('Run archive rollback stopped to preserve a newly created artifact; inspect the archive', { cause: error });
      }
      await fs.rename(destination, file);
    }
    throw new Error('Could not archive previous run artifacts; rerun was stopped', { cause: error });
  }
  return directory;
}

// The newest archived REVIEW.md lets a rerun after an interrupted run still start from the last review findings.
export async function latestArchivedReview(worktree, { task, git, accept = () => true }) {
  if (typeof task !== 'string' || !/^(?:issue-[1-9]\d*|local-[a-f0-9]{16})$/.test(task)) return null;
  const common = path.resolve(worktree, (await git(['rev-parse', '--git-common-dir'])).trim());
  const root = path.join(common, 'roster-artifacts', task);
  const entries = await fs.readdir(root).catch(() => []);
  for (const name of entries.filter((entry) => /^\d+-[a-f0-9]{12}$/.test(entry)).sort().reverse()) {
    const file = path.join(root, name, 'REVIEW.md');
    const stat = await fs.lstat(file).catch(() => null);
    if (!stat?.isFile() || stat.isSymbolicLink() || stat.size > 64 * 1024) continue;
    const text = await fs.readFile(file, 'utf8');
    if (accept(text)) return text;
  }
  return null;
}

// Scope the harness recorded for this task in earlier runs (regression-repaired tests and reviewed expansions),
// so a resumed run's gate does not reject an earlier context's legitimate edits that are still in the worktree.
export async function archivedRunScope(worktree, { task, git, limit = 50 }) {
  const scope = { repairFiles: [], scopeFiles: [] };
  if (typeof task !== 'string' || !/^(?:issue-[1-9]\d*|local-[a-f0-9]{16})$/.test(task)) return scope;
  const common = path.resolve(worktree, (await git(['rev-parse', '--git-common-dir'])).trim());
  const root = path.join(common, 'roster-artifacts', task);
  const entries = await fs.readdir(root).catch(() => []);
  const repair = new Set();
  const expanded = new Set();
  for (const name of entries.filter((entry) => /^\d+-[a-f0-9]{12}$/.test(entry)).sort().reverse().slice(0, limit)) {
    const file = path.join(root, name, 'RESULT.md');
    const stat = await fs.lstat(file).catch(() => null);
    if (!stat?.isFile() || stat.isSymbolicLink() || stat.size > 256 * 1024) continue;
    for (const line of (await fs.readFile(file, 'utf8')).split(/\r?\n/)) {
      const match = line.match(/^(Additional failing-test scope|Files outside planned scope): (.+)$/);
      if (!match || match[2] === '(none)') continue;
      for (const entry of match[2].split(', ')) (match[1].startsWith('Additional') ? repair : expanded).add(entry.trim());
    }
  }
  scope.repairFiles = [...repair].sort();
  scope.scopeFiles = [...expanded].sort();
  return scope;
}
