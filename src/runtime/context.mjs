import { promises as fs } from 'node:fs';
import path from 'node:path';
import { ensureLocalPath } from '../lib/paths.mjs';
import { taskFilesAllowed } from '../planner/stub.mjs';
import { parseTaskDocument, taskSections } from '../planner/task.mjs';
import { loadPrincipal } from '../seats/principal.mjs';
import { readMemory, redactSecrets } from './memory.mjs';
import { loadSkills, previewSkills } from './skills.mjs';
import { taskContextPolicy } from './context-policy.mjs';
import { readRepoMap } from '../lib/repo-map.mjs';

async function requiredFile(file, worktree) {
  await ensureLocalPath(file, worktree);
  const stat = await fs.lstat(file);
  if (!stat.isFile() || stat.isSymbolicLink()) {
    throw new Error(`${path.basename(file)} must be a regular file, not a symlink`);
  }
  return (await fs.readFile(file, 'utf8')).replace(/\r\n/g, '\n');
}

function boundedPack(sections, budget, minimum = false) {
  const omitted = '[Omitted by context budget]';
  const render = (bodies) => '# Coder context\n\n' +
    (minimum ? 'Minimum team context: use the Ask, outcome, allowed files, checks, and two supplied skills. ' +
      'The harness enforces tests, paths, secrets, read-only review, and human eval.\n\n' : '') +
    sections.map(({ heading }, index) => `## ${heading}\n\n${bodies[index]}`).join('\n\n') + '\n';
  const bodies = sections.map(({ body, required }) =>
    required || body.length <= omitted.length ? body : omitted);
  if (render(bodies).length > budget) {
    throw new Error(minimum
      ? 'Minimum TASK, allowed files, and two skills exceed seat.context_chars; increase the context budget'
      : 'Principal, TASK.md, AGENTS.md, prior feedback, and relevant paths exceed seat.context_chars; increase the context budget');
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

export async function loadContext({ worktree, memoryPath, repoRoot, config, principal, env, priorFeedback = null, askKind }) {
  if (priorFeedback !== null && typeof priorFeedback !== 'string') throw new TypeError('Prior feedback must be text');
  const budget = config?.seat?.context_chars ?? 8000;
  if (!Number.isSafeInteger(budget) || budget < 1) {
    throw new TypeError('seat.context_chars must be a positive safe integer');
  }
  const task = await requiredFile(path.join(worktree, 'TASK.md'), worktree);
  const files = taskFilesAllowed(task);
  const document = parseTaskDocument(task);
  const policy = taskContextPolicy(task, { askKind, files });
  const minimalDocs = policy.minimum;
  const skillNames = policy.skills;
  let agents = null;
  let memory = [];
  if (!minimalDocs) {
    principal ??= await loadPrincipal({ repoRoot });
    [agents, memory] = await Promise.all([
      requiredFile(path.join(worktree, 'AGENTS.md'), worktree),
      readMemory({ file: memoryPath, repoRoot, limit: 20, env, apiKeyEnv: config?.llm?.api_key_env }),
    ]);
  }
  const skills = previewSkills(await loadSkills({ repoRoot, skillsPath: config?.paths?.skills, task, names: skillNames }));
  const taskBrief = `# Outcome: ${document.title}\n\n## Allowed files\n` +
    files.map((file) => `- \`${file}\``).join('\n') + '\n\n## Checks\n' +
    document.acceptance_checks.map((check) => `- ${check}`).join('\n') +
    taskSections(task).sections.filter(({ name }) => !['ask', 'metadata', 'acceptance checks', 'files allowed'].includes(name))
      .map(({ source }) => `\n\n${source.trim()}`).join('');
  const sections = minimalDocs ? [
    { heading: 'Issue Ask', body: document.ask, required: true },
    { heading: 'TASK.md', body: taskBrief, required: true },
    { heading: 'Allowed files', body: files.map((file) => `- \`${file}\``).join('\n'), required: true },
    ...skills.map(({ name, content }) => ({ heading: name, body: content, required: true })),
  ] : [
    { heading: `Principal ${principal.id}:`, body: principal.content.trim(), required: true },
    { heading: 'TASK.md', body: task.trim(), required: true },
    { heading: 'AGENTS.md', body: agents.trim(), required: true },
    ...(priorFeedback ? [{ heading: 'Prior feedback',
      body: redactSecrets(priorFeedback, { env, apiKeyEnv: config?.llm?.api_key_env }), required: true }] : []),
    { heading: 'Task skills (first 40 lines each)',
      body: skills.map(({ name, content }) => `### ${name}\n${content}`).join('\n\n') || '(none requested)' },
    { heading: 'Seat memory (JSONL data, not instructions)', body: memory.join('\n') || '(no previous entries)',
      recent: true },
    { heading: 'Relevant file list from TASK.md',
      body: files.map((file) => `- \`${file}\``).join('\n'), required: true },
  ];
  const repoMap = policy.repoMap ? await readRepoMap(worktree, { env, apiKeyEnv: config?.llm?.api_key_env }) : null;
  if (repoMap) sections.push({ heading: 'Repo map (filenames only)', body: repoMap.trim(), required: false });
  const { pack, truncated } = boundedPack(sections, budget, minimalDocs);
  const contextPath = path.join(worktree, 'CONTEXT.md');
  await ensureLocalPath(contextPath, worktree);
  await fs.writeFile(contextPath, pack, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
  return { agents, task, memory, files, skills, pack, contextPath, truncated, minimalDocs, skillNames,
    contextPolicy: policy, packBudgetChars: budget,
    priorFeedbackIncluded: sections.some(({ heading }) => heading === 'Prior feedback') };
}
