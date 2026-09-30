import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { loadConfig } from './config.mjs';
import { githubRepository } from './issue.mjs';
import { repositoryRoot } from './learn.mjs';
import { ensureLocalPath } from './paths.mjs';
import { readLastRunLog } from './run-log.mjs';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));
const execFileAsync = promisify(execFile);

async function execute(program, args, cwd) {
  const { stdout } = await execFileAsync(program, args, {
    cwd, encoding: 'utf8', timeout: 30_000, maxBuffer: 1024 * 1024,
    env: { ...process.env, GH_PROMPT_DISABLED: '1' },
  });
  return stdout;
}

function issueNumber(value) {
  if (!/^[1-9]\d*$/.test(String(value)) || !Number.isSafeInteger(Number(value))) {
    throw new TypeError('Use roster status --issue N [--offline] with a positive safe issue number.');
  }
  return Number(value);
}

async function cachedIssue(worktreePath, number) {
  const file = path.join(worktreePath, 'ASSIGNMENT.md');
  let stat;
  try {
    stat = await fs.lstat(file);
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
  if (!stat.isFile() || stat.isSymbolicLink()) throw new Error('ASSIGNMENT.md must be a regular file');
  const assignment = await fs.readFile(file, 'utf8');
  const recorded = /^- Issue number: ([1-9]\d*)$/m.exec(assignment);
  const title = /^- Title: (.+)$/m.exec(assignment);
  const url = /^- Issue URL: (https:\/\/github\.com\/[A-Za-z0-9-]+\/[A-Za-z0-9_.-]+\/issues\/[1-9]\d*)$/m.exec(assignment);
  if (!recorded || Number(recorded[1]) !== number || !title || !url ||
      !url[1].endsWith(`/issues/${number}`)) {
    throw new Error('ASSIGNMENT.md does not match the requested issue');
  }
  return { number, title: title[1], url: url[1], state: 'UNKNOWN' };
}

export async function readStatus({
  issue: requestedIssue,
  offline = false,
  cwd = process.cwd(),
  repoRoot,
  config = loadConfig({ repoRoot: rosterRoot }),
  runCommand = execute,
  env = process.env,
} = {}) {
  if (typeof offline !== 'boolean') throw new TypeError('offline must be a boolean');
  const root = repoRoot === undefined ? repositoryRoot(cwd) : path.resolve(repoRoot);
  let number;
  if (requestedIssue === undefined) {
    const branch = (await runCommand('git', ['branch', '--show-current'], root)).trim();
    const match = /^issue-([1-9]\d*)$/.exec(branch);
    if (match) number = issueNumber(match[1]);
    else {
      let entries;
      try {
        entries = await fs.readdir(path.join(root, config.paths.worktrees), { withFileTypes: true });
      } catch (error) {
        if (error.code !== 'ENOENT') throw error;
        entries = [];
      }
      const matches = entries.filter((entry) => entry.isDirectory() && /^issue-[1-9]\d*$/.test(entry.name));
      if (matches.length !== 1) {
        throw new TypeError('Use roster status --issue N [--offline] when there is not exactly one issue worktree.');
      }
      number = issueNumber(matches[0].name.slice('issue-'.length));
    }
  } else {
    number = issueNumber(requestedIssue);
  }
  const worktreePath = path.join(root, config.paths.worktrees, `issue-${number}`);
  await ensureLocalPath(worktreePath, root);
  let worktreeExists = false;
  try {
    const stat = await fs.lstat(worktreePath);
    if (!stat.isDirectory() || stat.isSymbolicLink()) {
      throw new Error('Issue worktree must be a real directory');
    }
    worktreeExists = true;
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  const localIssue = worktreeExists ? await cachedIssue(worktreePath, number) : null;
  const runLog = await readLastRunLog({
    repoRoot: root, session: `roster-${number}-coder`, env, apiKeyEnv: config.llm.api_key_env,
  });
  if (offline) {
    return {
      issue: localIssue ?? { number, title: null, url: null, state: 'UNKNOWN' },
      openPr: undefined, worktreePath, worktreeExists, offline: true,
      ...(runLog ? { runLog } : {}),
    };
  }
  const origin = await runCommand('git', ['remote', 'get-url', 'origin'], root);
  if (typeof origin !== 'string' || !origin.trim()) {
    throw new Error('git remote get-url origin returned no GitHub repository');
  }
  const repository = githubRepository(origin.trim());
  const rawIssue = await runCommand('gh', [
    'issue', 'view', String(number), '--repo', repository, '--json', 'number,title,state,url',
  ], root);
  let issue;
  try {
    issue = JSON.parse(rawIssue);
  } catch {
    throw new Error(`gh issue view ${number} returned invalid JSON`);
  }
  if (issue?.number !== number || typeof issue.title !== 'string' || !issue.title ||
      !['OPEN', 'CLOSED'].includes(issue.state) || typeof issue.url !== 'string' ||
      issue.url.toLowerCase() !== `https://github.com/${repository}/issues/${number}`.toLowerCase()) {
    throw new Error(`gh issue view ${number} returned incomplete issue details`);
  }
  const rawPrs = await runCommand('gh', [
    'pr', 'list', '--repo', repository, '--state', 'open', '--head', `issue-${number}`,
    '--limit', '2', '--json', 'number,title,url,headRefName',
  ], root);
  let prs;
  try {
    prs = JSON.parse(rawPrs);
  } catch {
    throw new Error('gh pr list returned invalid JSON');
  }
  if (!Array.isArray(prs) || prs.length > 1 || prs.some((pr) =>
    !Number.isSafeInteger(pr?.number) || pr.number <= 0 ||
    typeof pr.title !== 'string' || !pr.title ||
    pr.headRefName !== `issue-${number}` ||
    typeof pr.url !== 'string' ||
    pr.url.toLowerCase() !== `https://github.com/${repository}/pull/${pr.number}`.toLowerCase())) {
    throw new Error(`gh pr list returned invalid open PRs for issue #${number}`);
  }
  return { issue, openPr: prs[0] ?? null, worktreePath, worktreeExists, offline: false,
    ...(runLog ? { runLog } : {}) };
}

export function formatStatus(status) {
  if (!status?.issue || typeof status.worktreePath !== 'string') {
    throw new TypeError('Expected an issue status with a worktree path');
  }
  const issue = status.issue.title
    ? `#${status.issue.number} ${status.issue.title} (${status.issue.state}) ${status.issue.url}`
    : `#${status.issue.number} (not cached offline)`;
  const pr = status.offline ? 'unknown (offline)' : status.openPr
    ? `#${status.openPr.number} ${status.openPr.title} ${status.openPr.url}` : 'none';
  return `Issue: ${issue}\nOpen PR: ${pr}\n` +
    `Worktree: ${status.worktreePath} (${status.worktreeExists ? 'present' : 'missing'})\n` +
    `Last seat: ${status.runLog?.lastSeat ?? 'unknown (no run log)'}\n` +
    `Last log line: ${status.runLog?.lastLine ?? 'none'}\n`;
}
