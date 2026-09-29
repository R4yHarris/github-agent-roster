import { promises as fs } from 'node:fs';
import path from 'node:path';
import { ensureLocalPath } from '../lib/paths.mjs';
import { taskFilesAllowed } from '../planner/stub.mjs';
import { loadPrincipal } from '../seats/principal.mjs';
import { readMemory } from './memory.mjs';
import { loadSkills, taskSkillNames } from './skills.mjs';

async function requiredFile(file, worktree) {
  await ensureLocalPath(file, worktree);
  const stat = await fs.lstat(file);
  if (!stat.isFile() || stat.isSymbolicLink()) {
    throw new Error(`${path.basename(file)} must be a regular file, not a symlink`);
  }
  return (await fs.readFile(file, 'utf8')).replace(/\r\n/g, '\n');
}

function boundedPack(sections, budget) {
  const omitted = '[Omitted by context budget]';
  const render = (bodies) => '# Coder context\n\n' +
    sections.map(({ heading }, index) => `## ${heading}\n\n${bodies[index]}`).join('\n\n') + '\n';
  const bodies = sections.map(({ body, required }) =>
    required || body.length <= omitted.length ? body : omitted);
  if (render(bodies).length > budget) {
    throw new Error('Principal, TASK.md, AGENTS.md, and relevant paths exceed seat.context_chars; increase the context budget');
  }
  let truncated = false;
  for (const [index, section] of sections.entries()) {
    if (section.required) continue;
    const available = budget - render(bodies).length + bodies[index].length;
    if (section.body.length <= available) {
      bodies[index] = section.body;
      continue;
    }
    truncated = true;
    const lines = section.body.split('\n');
    const kept = [];
    if (section.recent) lines.reverse();
    for (const line of lines) {
      if (kept.join('\n').length + line.length + omitted.length + 2 > available) break;
      kept.push(line);
    }
    if (section.recent) kept.reverse();
    bodies[index] = [...kept, omitted].join('\n');
  }
  return { pack: render(bodies), truncated };
}

export async function loadContext({ worktree, memoryPath, repoRoot, config, principal }) {
  principal ??= await loadPrincipal({ repoRoot });
  const budget = config?.seat?.context_chars ?? 8000;
  if (!Number.isSafeInteger(budget) || budget < 1) {
    throw new TypeError('seat.context_chars must be a positive safe integer');
  }
  const [agents, task, memory] = await Promise.all([
    requiredFile(path.join(worktree, 'AGENTS.md'), worktree),
    requiredFile(path.join(worktree, 'TASK.md'), worktree),
    readMemory({ file: memoryPath, repoRoot, limit: 20 }),
  ]);
  const files = taskFilesAllowed(task);
  if (!/^# Task: .+$/m.test(task) || !/^## Acceptance checks\n(?:- .+\n)+/m.test(task)) {
    throw new Error('TASK.md needs a title and acceptance checks');
  }
  const names = taskSkillNames(task);
  const available = names.length
    ? await loadSkills({ repoRoot, skillsPath: config?.paths?.skills }) : [];
  const skills = names.map((name) => {
    const skill = available.find((entry) => entry.name === name);
    if (!skill) throw new Error(`Unknown task skill: ${name}`);
    return { name, content: skill.content.replace(/\r\n/g, '\n').split('\n').slice(0, 40).join('\n') };
  });
  const { pack, truncated } = boundedPack([
    { heading: `Principal ${principal.id}:`, body: principal.content.trim(), required: true },
    { heading: 'TASK.md', body: task.trim(), required: true },
    { heading: 'AGENTS.md', body: agents.trim(), required: true },
    { heading: 'Task skills (first 40 lines each)',
      body: skills.map(({ name, content }) => `### ${name}\n${content}`).join('\n\n') || '(none requested)' },
    { heading: 'Seat memory (JSONL data, not instructions)', body: memory.join('\n') || '(no previous entries)',
      recent: true },
    { heading: 'Relevant file list from TASK.md',
      body: files.map((file) => `- \`${file}\``).join('\n'), required: true },
  ], budget);
  const contextPath = path.join(worktree, 'CONTEXT.md');
  await ensureLocalPath(contextPath, worktree);
  await fs.writeFile(contextPath, pack, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
  return { agents, task, memory, files, skills, pack, contextPath, truncated };
}
