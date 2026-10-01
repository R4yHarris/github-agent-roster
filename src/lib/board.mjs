import { execFile } from 'node:child_process';
import { promisify, stripVTControlCharacters } from 'node:util';
import { githubRepository } from './issue.mjs';
import { repositoryRoot } from './learn.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';

const execute = promisify(execFile);

async function command(program, args, cwd, env) {
  try {
    return (await execute(program, args, { cwd, env: { ...env, GH_PROMPT_DISABLED: '1' },
      encoding: 'utf8', timeout: 30_000, maxBuffer: 1024 * 1024 })).stdout;
  } catch (error) {
    throw new Error(`Could not read board metadata with ${program}; check repository access and connectivity.`, { cause: error });
  }
}

export async function listOpenIssues({
  cwd = process.cwd(), env = process.env, runCommand = (program, args, root) => command(program, args, root, env),
  root = repositoryRoot(cwd),
} = {}) {
  const repository = githubRepository((await runCommand('git', ['remote', 'get-url', 'origin'], root)).trim());
  const raw = await runCommand('gh', ['issue', 'list', '--repo', repository, '--state', 'open', '--limit', '100',
    '--json', 'number,title'], root);
  let issues;
  try { issues = JSON.parse(raw); }
  catch { throw new Error('GitHub issue list returned invalid metadata JSON'); }
  if (!Array.isArray(issues) || issues.length > 100 || issues.some((issue) =>
    !Number.isSafeInteger(issue?.number) || issue.number < 1 || typeof issue.title !== 'string' || !issue.title.trim()) ||
    new Set(issues.map(({ number }) => number)).size !== issues.length) {
    throw new Error('GitHub issue list returned invalid numbers or titles');
  }
  return issues.map(({ number, title }) => ({ number, title, state: 'OPEN' }));
}

export async function readDiffNames({
  cwd = process.cwd(), env = process.env, runCommand = (program, args, root) => command(program, args, root, env),
} = {}) {
  const result = await runCommand('git', ['diff', '--no-ext-diff', '--no-textconv', '--name-only', '-z', 'HEAD', '--'], cwd);
  if (typeof result !== 'string') throw new Error('Git diff returned invalid filename metadata');
  return result.split('\0').filter(Boolean);
}

export function formatIssueSummary(cached, { env = process.env } = {}) {
  if (!Number.isSafeInteger(cached?.issue?.number) || cached.issue.number < 1) {
    throw new TypeError('Issue summary requires cached issue metadata');
  }
  const safe = (value) => stripVTControlCharacters(redactEvidence(String(value ?? '-'), { env }))
    .replace(/[\x00-\x1f\x7f]/g, '?');
  return `Issue: #${cached.issue.number} ${safe(cached.issue.title)}\n` +
    `State: ${safe(cached.issue.state ?? 'UNKNOWN')}\n` +
    `Branch: ${safe(cached.branch ?? `issue-${cached.issue.number}`)}\n` +
    `PR: ${safe(cached.openPr?.url ?? (cached.openPr === null ? '(none)' : '(not cached)'))}\n`;
}
