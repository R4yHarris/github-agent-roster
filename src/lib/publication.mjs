import path from 'node:path';
import { resolvePublishModel } from '../metrics/run.mjs';
import { issueMergeMessage } from './issue-board.mjs';

export function buildPublishMessage({ subject, model, summary, testsSkipped = false, issueNumber }) {
  if (typeof subject !== 'string' || !subject.trim() || /[\r\n\0]/.test(subject)) {
    throw new TypeError('Publish subject must be nonempty single-line text');
  }
  if (typeof summary !== 'string' || !summary.trim() || summary.includes('\0')) {
    throw new TypeError('Publish summary must describe the reviewed changes');
  }
  const actualModel = resolvePublishModel({ env: { AI_MODEL: model } });
  const testing = testsSkipped
    ? 'Automatic tests were explicitly waived by TASK.md (`tests: none`). Review the task acceptance checks.'
    : 'Run `node --test` from the feature worktree root and review the task acceptance checks.';
  const message = `${subject.trim()}\n\n## Model\n\n${actualModel}\n\n## Summary\n\n${summary.trim()}` +
    `\n\n### How to test\n\n${testing}`;
  return issueNumber === undefined ? message : issueMergeMessage(message, issueNumber);
}

export function formatPublishCommand({
  message, model, script = path.join('vendor', 'github-agent-contracts', 'scripts', 'agent-pr.mjs'),
  platform = process.platform,
}) {
  const quote = platform === 'win32'
    ? (text) => `'${text.replaceAll("'", "''")}'`
    : (text) => `'${text.replaceAll("'", "'\\''")}'`;
  const actualModel = resolvePublishModel({ env: { AI_MODEL: model } });
  return `node ${script} --message ${quote(message)} --model ${actualModel} --merge-when-green`;
}
