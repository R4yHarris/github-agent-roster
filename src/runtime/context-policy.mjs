import { readTaskMetadata } from './estimate.mjs';
import { taskFilesAllowed } from '../planner/task.mjs';

export const minimumDocsSkills = Object.freeze(['read-before-write', 'small-diff']);

export function isMinimumDocsTask(task) {
  const metadata = readTaskMetadata(task);
  return metadata.task_class === 'docs' && metadata.difficulty === 1;
}

export function taskContextPolicy(task, { askKind, files } = {}) {
  const metadata = readTaskMetadata(task);
  const research = askKind !== 'slice' && metadata.task_class === 'feat' && metadata.difficulty >= 4;
  const allowed = metadata.task_class === 'docs' && metadata.difficulty === 1
    ? files ?? taskFilesAllowed(task) : [];
  const readmeOnlyDocs = allowed.length === 1 && allowed[0] === 'README.md';
  return { minimum: !research, research, readmeOnlyDocs, sliceReadsOnly: askKind === undefined || askKind === 'slice',
    repoMap: metadata.difficulty >= 4 && metadata.task_class !== 'docs',
    skills: research ? undefined : [...minimumDocsSkills] };
}
