import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { IDENTIFIER, inferTaskClass, recordRun } from './learn.mjs';
import { loadConfig } from './config.mjs';
import { buildPublishMessage, formatPublishCommand, formatPublishEnvironment } from './publication.mjs';
import { buildPublishEnv } from '../metrics/run.mjs';
import { cleanAskText, renderAssignment } from '../planner/stub.mjs';
import { estimateTask } from '../runtime/estimate.mjs';
import { initializeWorktreeSubmodules } from './contracts.mjs';
import { githubRepository } from './github-repository.mjs';
export { githubRepository } from './github-repository.mjs';

const execFileAsync = promisify(execFile);
const metadataMarker = '\n\n## Task metadata\n\n';

async function execute(program, args, cwd) {
  const { stdout } = await execFileAsync(program, args, { cwd, encoding: 'utf8' });
  return stdout;
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

export function validateIssueNumber(issueNumber) {
  if (!/^[1-9]\d*$/.test(String(issueNumber)) || !Number.isSafeInteger(Number(issueNumber))) {
    throw new TypeError('Issue number must be a positive safe integer');
  }
  return Number(issueNumber);
}

export async function runIssue(issueNumber, {
  cwd = process.cwd(),
  runCommand = execute,
  fileSystem = fs,
  env = process.env,
  config = loadConfig({ cwd }),
  now = () => new Date(),
  log = console.log,
  worktrees = '.worktrees',
  beforeWorktree = async () => {},
  sessionId,
  recordPreparation = true,
} = {}) {
  const number = validateIssueNumber(issueNumber);
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
  const publishEnv = recordPreparation && config.publish?.enabled !== false && env.AI_MODEL
    ? buildPublishEnv({ config, env, task }) : null;
  const model = publishEnv?.AI_MODEL;
  const nextCommand = model ? formatPublishCommand({
    model,
    message: buildPublishMessage({
      subject: `feat: issue ${number}`, model, summary: issue.title, issueNumber: number, ghcp: true,
    }),
    script: process.platform === 'win32'
      ? '"$env:GITHUB_AGENT_CONTRACTS\\scripts\\agent-pr.mjs"'
      : '"$GITHUB_AGENT_CONTRACTS/scripts/agent-pr.mjs"',
  }) : null;

  const inventory = await command('git', ['worktree', 'list', '--porcelain', '-z'], repoRoot);
  const registered = inventory.split('\0\0').filter(Boolean).map((record) => {
    const fields = record.split('\0');
    return {
      worktree: fields.find((field) => field.startsWith('worktree '))?.slice(9),
      branch: fields.find((field) => field.startsWith('branch '))?.slice(7),
    };
  });
  const samePath = (value) => typeof value === 'string' &&
    (process.platform === 'win32' ? path.resolve(value).toLowerCase() : path.resolve(value)) ===
    (process.platform === 'win32' ? worktreePath.toLowerCase() : worktreePath);
  const existing = registered.find(({ worktree }) => samePath(worktree));
  const branch = `refs/heads/${task}`;
  const reused = Boolean(existing);
  if (existing && existing.branch !== branch) throw new Error('Existing issue worktree is on a different branch');
  if (registered.some((entry) => entry.branch === branch && !samePath(entry.worktree))) {
    throw new Error('Issue branch is already checked out in another worktree; refusing to move it');
  }
  const entry = await (fileSystem.lstat ? fileSystem.lstat(worktreePath) : fileSystem.stat(worktreePath))
    .catch((error) => {
      if (error.code === 'ENOENT') return null;
      throw error;
    });
  if (entry && (!entry.isDirectory() || entry.isSymbolicLink?.())) {
    throw new Error('Issue worktree must be a real directory, not a symlink');
  }
  if (reused && !entry) throw new Error('Registered issue worktree is missing; repair its Git registration before running');
  if (!reused && entry) throw new Error('Issue worktree path exists without matching Git registration; refusing to overwrite it');
  await beforeWorktree(repoRoot, worktreePath);
  if (!reused) {
    const knownBranch = await command('git', ['for-each-ref', '--format=%(refname)', branch], repoRoot);
    await fileSystem.mkdir(path.dirname(worktreePath), { recursive: true });
    await command('git', knownBranch.split('\n').includes(branch)
      ? ['worktree', 'add', worktreePath, task] : ['worktree', 'add', '-b', task, worktreePath], repoRoot);
  }
  await initializeWorktreeSubmodules(worktreePath, command);

  const assignment = renderAssignment(issue);
  const writeAssignment = async (file, content, options) => {
    try {
      await fileSystem.writeFile(file, content, options);
    } catch (error) {
      if (!reused || error.code !== 'EEXIST') throw error;
      const status = await (fileSystem.lstat ? fileSystem.lstat(file) : fileSystem.stat(file));
      if (!status.isFile() || status.isSymbolicLink?.()) {
        throw new Error('Existing assignment/environment must be a regular file');
      }
    }
  };
  try {
    await writeAssignment(assignmentPath, assignment, { encoding: 'utf8', flag: 'wx' });
    await writeAssignment(envPath, `AI_TASK=${task}\nAI_SESSION=${session}\n`, {
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
        cwd: repoRoot, env: publishEnv ?? env, fileSystem,
      });
    } catch (error) {
      throw new Error(`Worktree ${worktreePath} was prepared but run recording failed: ${error.message}`, { cause: error });
    }
  }

  log(`Worktree: ${worktreePath}${reused ? ' (reused)' : ''}
Assignment: ${assignmentPath}
Environment: ${envPath}
After editing inside the worktree, load .env into the worker environment and run:
${publishEnv ? `For GHCP publication, apply these values after loading .env (empty clears inherited fields):\n` +
  formatPublishEnvironment(publishEnv) : ''}
${nextCommand ?? (config.publish?.enabled === false
  ? 'Publication unavailable: publishing is disabled by publish.enabled.'
  : 'Publication unavailable: set model in AI_MODEL for GHCP, or complete a measured roster run.')}`);

  return { issue, ask, metadata, repoRoot, worktreePath, assignmentPath, envPath, task, session, nextCommand, reused };
}
