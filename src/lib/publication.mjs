import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { resolvePublishModel, RUN_ENV_NAMES } from '../metrics/run.mjs';
import { issueMergeMessage } from './issue-board.mjs';

export function parsePublishArgs(args) {
  if (typeof args !== 'string') throw new TypeError('Publish arguments must be text');
  const words = args.trim() ? args.trim().split(/\s+/) : [];
  const subject = [];
  let model;
  let skipReview = false;
  for (let index = 0; index < words.length; index += 1) {
    const word = words[index];
    if (word === '--model') {
      if (model !== undefined || !words[index + 1] || words[index + 1].startsWith('--')) {
        throw new TypeError('Use /publish [SUBJECT] [--model MODEL] [--skip-review]');
      }
      model = resolvePublishModel({ model: words[++index], env: {}, ghcp: true });
    } else if (word === '--skip-review') {
      if (skipReview) throw new TypeError('--skip-review may be supplied only once');
      skipReview = true;
    } else if (word.startsWith('--')) {
      throw new TypeError('Use /publish [SUBJECT] [--model MODEL] [--skip-review]');
    } else {
      subject.push(word);
    }
  }
  return { subject: subject.join(' ') || null, model, skipReview };
}

export function publicationTask({ cwd = process.cwd(), env = process.env, task, run = execFileSync } = {}) {
  const known = task ?? env.AI_TASK;
  if (known !== undefined && known !== '') {
    if (typeof known !== 'string' || !/^[A-Za-z0-9._-]{1,64}$/.test(known) || known === '-') {
      throw new TypeError('AI_TASK must be an opaque branch slug or issue identifier');
    }
    return known;
  }
  let branch;
  try {
    branch = run('git', ['symbolic-ref', '--short', 'HEAD'], {
      cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
    }).trim();
  } catch (error) {
    throw new Error('GHCP publication needs a checked-out branch or an explicit AI_TASK', { cause: error });
  }
  const slug = branch.replace(/[^A-Za-z0-9._-]+/g, '-').slice(0, 64);
  if (!slug || /^[-._]+$/.test(slug)) {
    throw new Error('Set AI_TASK: the branch cannot form an opaque publication slug');
  }
  return slug;
}

export function formatPublishEnvironment(env) {
  return RUN_ENV_NAMES.map((name) => `${name}=${env[name] ?? ''}\n`).join('');
}

export function buildPublishMessage({ subject, model, summary, testsSkipped = false, issueNumber, seats, ghcp = false }) {
  if (typeof subject !== 'string' || !subject.trim() || /[\r\n\0]/.test(subject)) {
    throw new TypeError('Publish subject must be nonempty single-line text');
  }
  if (typeof summary !== 'string' || !summary.trim() || summary.includes('\0')) {
    throw new TypeError('Publish summary must describe the reviewed changes');
  }
  if (seats !== undefined && (typeof seats !== 'string' || !seats.trim() ||
      /[\r\n\0]/.test(seats) || seats.length > 200)) {
    throw new TypeError('Publish seats must be one nonempty line');
  }
  const actualModel = resolvePublishModel({ env: { AI_MODEL: model } });
  const testing = testsSkipped
    ? 'Automatic tests were explicitly waived by TASK.md (`tests: none`). Review the task acceptance checks.'
    : 'Run `node --test` from the feature worktree root and review the task acceptance checks.';
  const message = `${subject.trim()}\n\n## Model\n\n${actualModel}\n\n## Summary\n\n${summary.trim()}` +
    (seats ? `\n\n## Seats\n\n${seats}` : '') +
    (ghcp ? '\n\n## Metrics\n\nGHCP used/out are `-` (unknown); declared context capacity is not token usage.' : '') +
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
