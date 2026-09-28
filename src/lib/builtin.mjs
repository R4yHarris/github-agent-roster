import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { buildRun, mergeUsage } from '../metrics/run.mjs';
import { planAsk, taskFilesAllowed } from '../planner/stub.mjs';
import { runCoder } from '../seats/coder.mjs';
import { isAllowedFile, isForbiddenWrite } from '../runtime/tools.mjs';
import { loadConfig } from './config.mjs';
import { runIssue } from './issue.mjs';
import { ensureLocalPath, resolveContractsPath } from './paths.mjs';

const execFileAsync = promisify(execFile);
const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));
const generated = new Set(['ASSIGNMENT.md', 'TASK.md', 'RECIPE.yml', 'RESULT.md']);
const runNames = [
  'AI_PROVIDER', 'AI_MODEL', 'AI_MODEL_VERSION', 'AI_EFFORT', 'AI_CONTEXT_USED',
  'AI_CONTEXT_MAX', 'AI_CONTEXT_OUT', 'AI_SESSION', 'AI_TASK',
];

async function git(worktree, args, env = process.env) {
  const { stdout } = await execFileAsync('git', args, {
    cwd: worktree, env, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024,
  });
  return stdout;
}

export async function stageReviewedFiles(worktree, allowedFiles, { env = process.env } = {}) {
  const status = await git(worktree,
    ['status', '--porcelain=v1', '--no-renames', '-z', '--untracked-files=all'], env);
  const files = [];
  for (const entry of status.split('\0').filter(Boolean)) {
    const file = entry.slice(3).replaceAll('\\', '/');
    if (!entry.startsWith('?? ') && !/^[ MADRCU?!]{2} /.test(entry.slice(0, 3))) {
      throw new Error('Could not parse worktree Git status before publishing');
    }
    if (generated.has(file)) continue;
    if (isForbiddenWrite(file) || !isAllowedFile(file, allowedFiles)) {
      throw new Error(`Refusing to publish file outside TASK.md scope or protected by policy: ${file}`);
    }
    files.push(file);
  }
  if (!files.length) throw new Error('No reviewed task files changed; nothing to publish');
  await git(worktree, ['add', '--all', '--', ...files], { ...env, GIT_LITERAL_PATHSPECS: '1' });
  const staged = (await git(worktree,
    ['diff', '--cached', '--name-only', '--no-renames', '-z'], env)).split('\0').filter(Boolean);
  if (staged.length !== files.length || staged.some((file) => !files.includes(file))) {
    throw new Error('Staged files changed unexpectedly; refusing to publish');
  }
  await git(worktree, ['diff', '--cached', '--check'], env);
  return files;
}

export async function runBuiltinIssue(issueNumber, {
  cwd = process.cwd(),
  repoRoot = rosterRoot,
  config = loadConfig({ repoRoot }),
  env = process.env,
  publish = false,
  log = console.log,
  runCommand,
  fetchImpl,
  runTestCommand,
  publisher = execFileAsync,
  now,
} = {}) {
  if (publish && (!config.llm.base_url || !env.GITHUB_APP_ID || !env.GITHUB_APP_PRIVATE_KEY_PATH)) {
    throw new Error('--publish requires an LLM endpoint and GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH');
  }
  const commandEnv = { ...env };
  delete commandEnv[config.llm.api_key_env];
  resolveContractsPath({ repoRoot, cwd, env });
  const issueCommand = runCommand ?? (async (program, args, workingDirectory) =>
    (await execFileAsync(program, args, { cwd: workingDirectory, env: commandEnv, encoding: 'utf8' })).stdout);
  const prepared = await runIssue(issueNumber, {
    cwd, runCommand: issueCommand, worktrees: config.paths.worktrees, log: () => {}, now,
    beforeWorktree: (root, worktreePath) => ensureLocalPath(worktreePath, root),
  });
  const { worktreePath } = prepared;
  const plan = await planAsk(prepared.issue.body, {
    config, reference: `issue:${prepared.issue.number}`, title: prepared.issue.title, fetchImpl, env,
  });
  const recipePath = path.join(worktreePath, 'RECIPE.yml');
  const taskPath = path.join(worktreePath, 'TASK.md');
  await fs.writeFile(recipePath, plan.recipe, { encoding: 'utf8', flag: 'wx' });
  await fs.writeFile(taskPath, plan.task, { encoding: 'utf8', flag: 'wx' });
  const result = await runCoder({
    worktree: worktreePath, repoRoot, config, task: prepared.task, session: prepared.session,
    fetchImpl, env, runTestCommand,
  });
  const run = buildRun({
    config, usage: config.llm.base_url ? mergeUsage(plan.usage, result.usage) : {},
    session: prepared.session, task: prepared.task,
  });
  const command = `node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "feat: issue ${prepared.issue.number}"`;
  log(`Worktree: ${worktreePath}\nAssignment: ${prepared.assignmentPath}\n` +
    `RECIPE: ${recipePath}\nTASK: ${taskPath}\nRESULT: ${result.resultPath}\n` +
    (run ? `AI-Run: ${run.line}\n` : '') +
    `From the worktree root, publish only after reviewing changes:\n${command}`);

  if (publish) {
    await git(worktreePath, ['submodule', 'update', '--init', '--recursive'], commandEnv);
    const contractsPath = resolveContractsPath({ repoRoot: worktreePath, cwd, env });
    await stageReviewedFiles(worktreePath, taskFilesAllowed(plan.task), { env: commandEnv });
    const publishEnv = { ...commandEnv };
    for (const name of runNames) delete publishEnv[name];
    publishEnv.GITHUB_APP_PRIVATE_KEY_PATH = path.resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH);
    Object.assign(publishEnv, run.env);
    const { stdout } = await publisher(process.execPath,
      [path.join(contractsPath, 'scripts', 'agent-pr.mjs'),
        '--message', `feat: issue ${prepared.issue.number}`],
      { cwd: worktreePath, env: publishEnv, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 });
    if (stdout?.trim()) log(stdout.trim());
  }
  return { ...prepared, recipePath, taskPath, result, run, command };
}
