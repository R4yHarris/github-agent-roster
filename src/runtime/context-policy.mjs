import { skillCategories, taskSkillNames } from './skills.mjs';
import { readTaskMetadata } from './estimate.mjs';
import { normalizeAsk, parseTaskDocument, taskFilesAllowed } from '../planner/task.mjs';
import { classifyAsk } from '../planner/classify.mjs';
import { askRequirements, cleanAskText } from '../planner/stub.mjs';

export const minimumDocsSkills = Object.freeze(['read-before-write', 'small-diff']);

export function isMinimumDocsTask(task) {
  const metadata = readTaskMetadata(task);
  return metadata.task_class === 'docs' && metadata.difficulty === 1;
}

export function coderTaskSkillNames(task) {
  const names = taskSkillNames(task);
  const document = parseTaskDocument(task);
  const classification = classifyAsk(document.ask, { filesAllowed: document.files_allowed });
  if (classification.kind !== 'slice' || classification.specKind !== 'incident') return names;
  const text = cleanAskText(document.ask);
  const lead = text.replace(/<!--[\s\S]*?-->/g, '').split('\n').map(normalizeAsk).find(Boolean);
  const directives = [lead, ...askRequirements(text, { allowMissing: true }).outcomes ?? []].filter(Boolean);
  // A taxonomy label or emergency description is not itself an instruction to act.
  const authorized = directives.some((line) =>
    /^(?:fix|repair|triage|diagnose|investigate|reproduce|resolve)\b/i.test(normalizeAsk(line)));
  return authorized && !names.includes('incident') ? [...names, 'incident'] : names;
}

export function taskContextPolicy(task, { askKind, files } = {}) {
  const metadata = readTaskMetadata(task);
  const research = askKind !== 'slice' && metadata.task_class === 'feat' && metadata.difficulty >= 4;
  const allowed = metadata.task_class === 'docs' && metadata.difficulty === 1
    ? files ?? taskFilesAllowed(task) : [];
  const readmeOnlyDocs = allowed.length === 1 && allowed[0] === 'README.md';
  const skills = coderTaskSkillNames(task);
  return { minimum: !research, research, readmeOnlyDocs, sliceReadsOnly: askKind === undefined || askKind === 'slice',
    repoMap: metadata.difficulty >= 4 && metadata.task_class !== 'docs',
    skills: research ? skills : [...minimumDocsSkills, ...skills.filter((name) => skillCategories.includes(name))] };
}
