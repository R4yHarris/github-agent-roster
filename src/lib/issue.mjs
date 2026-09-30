import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { IDENTIFIER, inferTaskClass, recordRun } from './learn.mjs';
import { loadConfig } from './config.mjs';
import { buildPublishMessage, formatPublishCommand } from './publication.mjs';
import { resolvePublishModel } from '../metrics/run.mjs';
import { cleanAskText, renderAssignment } from '../planner/stub.mjs';
import { estimateTask } from '../runtime/estimate.mjs';

const execFileAsync = promisify(execFile);
const metadataMarker = '\n\n## Task metadata\n\n';

async function execute(program, args, cwd) {
  const { stdout } = await execFileAsync(program, args, { cwd, encoding: 'utf8' });
  return stdout;
}

export function githubRepository(origin) {
  let url;
  try {
    url = new URL(origin.startsWith('git@github.com:')
      ? `ssh://git@github.com/${origin.slice('git@github.com:'.length)}`
      : origin);
  } catch {
    throw new Error('origin must be a GitHub HTTPS or SSH repository URL');
  }
  const match = /^\/([A-Za-z0-9-]+)\/([A-Za-z0-9_.-]+?)(?:\.git)?$/.exec(url.pathname);
  if (url.hostname !== 'github.com' || !['https:', 'ssh:'].includes(url.protocol) ||
      url.port || url.search || url.hash || url.password ||
      (url.protocol === 'https:' ? url.username : url.username !== 'git') ||
      !match || ['.', '..'].includes(match[2])) {
    throw new Error('origin must be a GitHub HTTPS or SSH repository URL');
  }
  return `${match[1]}/${match[2]}`;
}

export function renderIssueBody(ask, metadata = {}) {
  const text = cleanAskText(ask);
  if (/^## Task metadata\s*$/im.test(text)) {
    throw new TypeError('Ask must not contain the reserved Task metadata heading');
  }
  const estimate = estimateTask({
    task_class: inferTaskClass(text.split('\n')[0]) ?? 'feat', ...metadata,
  });
  return `# Ask\n\n${text}${metadataMarker}` +
    `task_class: ${estimate.task_class}\n` +
    `difficulty: ${estimate.difficulty}\n` +
    `estimate_min: ${estimate.estimate_min}\n`;
}

export function parseIssueBody(body) {
  if (typeof body !== 'string' || !body.trim()) {
    throw new TypeError('Issue body must contain an Ask');
  }
  const text = body.replace(/\r\n/g, '\n');
  const parts = text.split(metadataMarker);
  if (parts.length === 1) {
    if (/^## Task metadata\s*$/im.test(text)) {
      throw new TypeError('Issue Task metadata must contain task_class, difficulty, and estimate_min');
    }
    return { ask: cleanAskText(text.startsWith('# Ask\n\n') ? text.slice(7) : text),
      metadata: null };
  }
  const match = parts.length === 2 &&
    /^task_class: (feat|fix|docs|test)\ndifficulty: ([1-5])\nestimate_min: (0|[1-9]\d*)\n?$/.exec(parts[1]);
  if (!match || /^## Task metadata\s*$/im.test(parts[0])) {
    throw new TypeError('Issue Task metadata must contain task_class, difficulty, and estimate_min');
  }
  const ask = parts[0].startsWith('# Ask\n\n') ? parts[0].slice(7) : parts[0];
  const validated = estimateTask({
    task_class: match[1], difficulty: Number(match[2]), estimate_min: Number(match[3]),
  });
  return {
    ask: cleanAskText(ask),
    metadata: {
      task_class: validated.task_class, difficulty: validated.difficulty,
      estimate_min: validated.estimate_min,
    },
  };
}

export async function runIssue(issueNumber, {
  cwd = process.cwd(),
  runCommand = execute,
  fileSystem = fs,
  env = process.env,
  config = loadConfig(),
  now = () => new Date(),
  log = console.log,
  worktrees = '.worktrees',
  beforeWorktree = async () => {},
  sessionId,
  recordPreparation = true,
} = {}) {
  if (!/^[1-9]\d*$/.test(String(issueNumber)) || !Number.isSafeInteger(Number(issueNumber))) {
    throw new TypeError('Issue number must be a positive safe integer');
  }
  if (sessionId !== undefined && (typeof sessionId !== 'string' || !IDENTIFIER.test(sessionId))) {
    throw new TypeError('Session ID must be an opaque identifier of at most 64 characters');
  }
  if (typeof recordPreparation !== 'boolean') {
    throw new TypeError('recordPreparation must be a boolean');
  }
  if (typeof worktrees !== 'string' || !worktrees || path.isAbsolute(worktrees) ||
      path.win32.isAbsolute(worktrees) ||
      worktrees.split(/[\\/]/).some((part) => !part || part === '.' || part === '..')) {
    throw new TypeError('Worktrees path must be relative to the repository root');
  }
  const number = Number(issueNumber);

  async function command(program, args, workingDirectory) {
    try {
      return (await runCommand(program, args, workingDirectory)).trim();
    } catch (error) {
      throw new Error(`${program} ${args.join(' ')} failed in ${workingDirectory}: ${error.message}`, { cause: error });
    }
  }

  const root = await command('git', ['rev-parse', '--show-toplevel'], cwd);
  if (!root) {
    throw new Error(`git rev-parse --show-toplevel returned no repository root for ${cwd}`);
  }
  const repoRoot = path.resolve(root);

  const origin = await command('git', ['remote', 'get-url', 'origin'], repoRoot);
  const repository = githubRepository(origin);
  const rawIssue = await command('gh', [
    'issue', 'view', String(number), '--repo', repository, '--json', 'number,title,body,url',
  ], repoRoot);
  let issue;
  try {
    issue = JSON.parse(rawIssue);
  } catch (error) {
    throw new Error(`gh issue view ${number} returned invalid JSON`, { cause: error });
  }
  if (!issue || issue.number !== number) {
    throw new Error(`gh issue view ${number} did not return issue #${number}`);
  }
  if (typeof issue.title !== 'string' || !issue.title.trim() ||
      typeof issue.url !== 'string' || !issue.url.trim()) {
    throw new Error(`gh issue view ${number} returned incomplete issue details`);
  }
  if (typeof issue.body !== 'string' || !issue.body.trim()) {
    throw new Error(`Issue #${number} has no body to use as the Ask`);
  }
  const { ask, metadata } = parseIssueBody(issue.body);

  const task = `issue-${number}`;
  const worktreePath = path.join(repoRoot, worktrees, task);
  const assignmentPath = path.join(worktreePath, 'ASSIGNMENT.md');
  const envPath = path.join(worktreePath, '.env');
  const session = sessionId ?? `roster-${now().toISOString().replace(/[-:.]/g, '')}`;
  const model = config.llm.model || env.AI_MODEL || env.ROSTER_MODEL
    ? resolvePublishModel({ config, env }) : null;
  const nextCommand = model ? formatPublishCommand({
    model,
    message: buildPublishMessage({
      subject: `feat: issue ${number}`, model, summary: issue.title, issueNumber: number,
    }),
    script: process.platform === 'win32'
      ? '"$env:GITHUB_AGENT_CONTRACTS\\scripts\\agent-pr.mjs"'
      : '"$GITHUB_AGENT_CONTRACTS/scripts/agent-pr.mjs"',
  }) : null;

  await beforeWorktree(repoRoot, worktreePath);
  await fileSystem.mkdir(path.dirname(worktreePath), { recursive: true });
  await command('git', ['worktree', 'add', '-b', task, worktreePath], repoRoot);

  const assignment = renderAssignment(issue);
  try {
    await fileSystem.writeFile(assignmentPath, assignment, { encoding: 'utf8', flag: 'wx' });
    await fileSystem.writeFile(envPath, `AI_TASK=${task}\nAI_SESSION=${session}\n`, {
      encoding: 'utf8',
      flag: 'wx',
      mode: 0o600,
    });
  } catch (error) {
    throw new Error(`Worktree ${worktreePath} was created but assignment setup failed: ${error.message}`, { cause: error });
  }

  if (recordPreparation) {
    try {
      await recordRun({ session, task, task_class: metadata?.task_class ?? inferTaskClass(issue.title) }, {
        cwd: repoRoot, env, fileSystem,
      });
    } catch (error) {
      throw new Error(`Worktree ${worktreePath} was prepared but run recording failed: ${error.message}`, { cause: error });
    }
  }

  log(`Worktree: ${worktreePath}
Assignment: ${assignmentPath}
Environment: ${envPath}
After editing inside the worktree, load .env into the worker environment and run:
${nextCommand ?? 'Publication unavailable: set model in llm.model, AI_MODEL, or ROSTER_MODEL before publishing.'}`);

  return { issue, ask, metadata, repoRoot, worktreePath, assignmentPath, envPath, task, session, nextCommand };
}
