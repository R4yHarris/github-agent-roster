import { promises as fs } from 'node:fs';
import path from 'node:path';
import { ensureLocalPath } from '../lib/paths.mjs';

export function taskSkillNames(task) {
  if (typeof task !== 'string') throw new TypeError('TASK.md must be text');
  const text = task.replace(/\r\n/g, '\n');
  if (!text.startsWith('---\n')) return [];
  const end = text.indexOf('\n---\n', 3);
  if (end < 0) throw new Error('TASK.md frontmatter must end with ---');
  const lines = text.slice(4, end).split('\n');
  const entries = lines.flatMap((line, index) => /^skills:/.test(line) ? [index] : []);
  if (!entries.length) return [];
  if (entries.length !== 1) throw new Error('TASK.md must not repeat skills');
  const index = entries[0];
  const value = lines[index].slice('skills:'.length).trim();
  let names;
  if (value) {
    const list = /^\[([^\[\]]*)\]$/.exec(value);
    if (!list) throw new Error('TASK.md skills must be a list of skill names');
    names = list[1].trim() ? list[1].split(',').map((name) => name.trim()) : [];
  } else {
    names = [];
    for (const line of lines.slice(index + 1)) {
      if (/^\S/.test(line)) break;
      const item = /^  - (\S+)\s*$/.exec(line);
      if (!item) throw new Error('TASK.md skills must be a list of skill names');
      names.push(item[1]);
    }
  }
  names = names.map((name) => name.replace(/^(['"])(.*)\1$/, '$2'));
  if (names.some((name) => !/^[a-z][a-z0-9-]{0,63}$/.test(name)) ||
      new Set(names).size !== names.length) {
    throw new Error('TASK.md skills must contain distinct, simple skill names');
  }
  return names;
}

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
