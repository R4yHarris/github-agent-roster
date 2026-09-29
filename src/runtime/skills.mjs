import { promises as fs } from 'node:fs';
import path from 'node:path';
import { ensureLocalPath } from '../lib/paths.mjs';

export async function loadSkills({ repoRoot, skillsPath = 'skills' }) {
  const root = await fs.realpath(repoRoot);
  const directory = path.resolve(root, skillsPath);
  const relative = path.relative(root, directory);
  if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error('Skills must be loaded from this roster repository');
  }
  await ensureLocalPath(directory, root);
  let status;
  try {
    status = await fs.lstat(directory);
  } catch (error) {
    if (error.code === 'ENOENT') return [];
    throw error;
  }
  if (!status.isDirectory() || status.isSymbolicLink()) {
    throw new Error('Skills path must be a directory, not a symlink');
  }
  const skills = [];
  for (const entry of (await fs.readdir(directory, { withFileTypes: true }))
    .sort((left, right) => left.name.localeCompare(right.name))) {
    if (!entry.isDirectory()) continue;
    const file = path.join(directory, entry.name, 'SKILL.md');
    let skillStatus;
    try {
      skillStatus = await fs.lstat(file);
    } catch (error) {
      if (error.code === 'ENOENT') continue;
      throw error;
    }
    if (!skillStatus.isFile() || skillStatus.isSymbolicLink()) {
      throw new Error(`Skill ${entry.name} must contain a regular SKILL.md file`);
    }
    skills.push({ name: entry.name, content: await fs.readFile(file, 'utf8') });
  }
  return skills;
}
