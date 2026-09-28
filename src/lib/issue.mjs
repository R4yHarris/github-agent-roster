import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);

async function execute(program, args, cwd) {
  const { stdout } = await execFileAsync(program, args, { cwd, encoding: 'utf8' });
  return stdout;
}

function githubRepository(origin) {
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

export async function runIssue(issueNumber, {
  cwd = process.cwd(),
  runCommand = execute,
  fileSystem = fs,
  now = () => new Date(),
  log = console.log,
} = {}) {
  if (!/^[1-9]\d*$/.test(String(issueNumber)) || !Number.isSafeInteger(Number(issueNumber))) {
    throw new TypeError('Issue number must be a positive safe integer');
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

  const task = `issue-${number}`;
  const worktreePath = path.join(repoRoot, '.worktrees', task);
  const assignmentPath = path.join(worktreePath, 'ASSIGNMENT.md');
  const envPath = path.join(worktreePath, '.env');
  const session = `roster-${now().toISOString().replace(/[-:.]/g, '')}`;
  const nextCommand = `node $GITHUB_AGENT_CONTRACTS/scripts/agent-pr.mjs --message "feat: issue ${number}"`;

  await fileSystem.mkdir(path.dirname(worktreePath), { recursive: true });
  await command('git', ['worktree', 'add', '-b', task, worktreePath], repoRoot);

  const assignment = `# Assignment

- Issue URL: ${issue.url}
- Issue number: ${number}
- Title: ${issue.title}

## Ask

${issue.body}
`;
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

  log(`Worktree: ${worktreePath}
Assignment: ${assignmentPath}
Environment: ${envPath}
After editing inside the worktree, load .env into the worker environment and run:
${nextCommand}`);

  return { issue, worktreePath, assignmentPath, envPath, task, session, nextCommand };
}
