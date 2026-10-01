import { readTaskMetadata } from './estimate.mjs';

export const minimumDocsSkills = Object.freeze(['read-before-write', 'small-diff']);

export function isMinimumDocsTask(task) {
  const metadata = readTaskMetadata(task);
  return metadata.task_class === 'docs' && metadata.difficulty === 1;
}

export function taskContextPolicy(task, { askKind } = {}) {
  const metadata = readTaskMetadata(task);
  const research = askKind !== 'slice' && metadata.task_class === 'feat' && metadata.difficulty >= 4;
  return { minimum: !research, research,
    skills: research ? undefined : [...minimumDocsSkills] };
}
