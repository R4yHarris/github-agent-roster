import { readTaskMetadata } from './estimate.mjs';

export const minimumDocsSkills = Object.freeze(['read-before-write', 'small-diff']);

export function isMinimumDocsTask(task) {
  const metadata = readTaskMetadata(task);
  return metadata.task_class === 'docs' && metadata.difficulty === 1;
}
