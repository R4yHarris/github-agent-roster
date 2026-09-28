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
  const status = await fs.lstat(directory);
  if (!status.isDirectory() || status.isSymbolicLink()) {
    throw new Error('Skills path must be a directory, not a symlink');
  }
  const skills = [];
  for (const entry of (await fs.readdir(directory, { withFileTypes: true }))
    .sort((left, right) => left.name.localeCompare(right.name))) {
    if (!entry.isDirectory()) continue;
    const file = path.join(directory, entry.name, 'SKILL.md');
    const skillStatus = await fs.lstat(file);
    if (!skillStatus.isFile() || skillStatus.isSymbolicLink()) {
      throw new Error(`Skill ${entry.name} must contain a regular SKILL.md file`);
    }
    skills.push({ name: entry.name, content: await fs.readFile(file, 'utf8') });
  }
  if (!skills.length) throw new Error('No skills/*/SKILL.md files found in the roster repository');
  return skills;
}
