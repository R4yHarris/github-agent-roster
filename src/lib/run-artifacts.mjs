import { randomBytes } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { ensureLocalPath } from './paths.mjs';

const names = ['RECIPE.yml', 'TASK.md', 'ESTIMATE.md', 'CONTEXT.md', 'RESEARCH.md', 'RESULT.md', 'REVIEW.md'];

export async function archiveRunArtifacts(worktree, { task, git, preserve = [] }) {
  if (typeof task !== 'string' || !/^issue-[1-9]\d*$/.test(task) || typeof git !== 'function') {
    throw new TypeError('Run archive requires an issue identifier and Git helper');
  }
  if (!Array.isArray(preserve) || preserve.some((name) => !['RECIPE.yml', 'TASK.md'].includes(name))) {
    throw new TypeError('Run archive may preserve only validated recipe/task artifacts');
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
  const directory = path.join(common, 'roster-artifacts', task,
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
