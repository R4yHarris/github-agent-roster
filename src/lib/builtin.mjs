import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { buildRun } from '../metrics/run.mjs';
import { taskFilesAllowed } from '../planner/stub.mjs';
import { runCoder } from '../seats/coder.mjs';
import { runPlanner } from '../seats/planner.mjs';
import { isAllowedFile, isForbiddenWrite } from '../runtime/tools.mjs';
import { loadConfig } from './config.mjs';
import { runIssue } from './issue.mjs';
import { inferTaskClass, recordRun } from './learn.mjs';
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

async function ensureUnchanged(file, content) {
  const status = await fs.lstat(file);
  if (!status.isFile() || status.isSymbolicLink() || await fs.readFile(file, 'utf8') !== content) {
    throw new Error(`${path.basename(file)} changed after planning; refusing to publish`);
  }
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

export async function prepareBuiltinPublication(run, {
  cwd = process.cwd(),
  config = loadConfig({ repoRoot: rosterRoot }),
  env = process.env,
} = {}) {
  if (typeof run?.worktreePath !== 'string' || typeof run.planner?.recipe !== 'string' ||
      typeof run.planner?.task !== 'string' || !run.runs?.coder?.env) {
    throw new TypeError('Publishing requires a completed builtin run');
  }
  if (run.result?.mode !== 'llm' || run.result.tests?.exit_code !== 0) {
    throw new Error('Publishing requires a configured coder run with passing tests');
  }
  if (!env.GITHUB_APP_ID || !env.GITHUB_APP_PRIVATE_KEY_PATH) {
    throw new Error('Publishing requires GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH');
  }
  await ensureUnchanged(run.recipePath, run.planner.recipe);
  await ensureUnchanged(run.taskPath, run.planner.task);
  const commandEnv = { ...env };
  delete commandEnv[config.llm.api_key_env];
  await git(run.worktreePath, ['submodule', 'update', '--init', '--recursive'], commandEnv);
  const contractsPath = resolveContractsPath({ repoRoot: run.worktreePath, cwd, env });
  await stageReviewedFiles(run.worktreePath, taskFilesAllowed(run.planner.task), { env: commandEnv });
  const publishEnv = { ...commandEnv };
  for (const name of runNames) delete publishEnv[name];
  publishEnv.GITHUB_APP_PRIVATE_KEY_PATH = path.resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH);
  Object.assign(publishEnv, run.runs.coder.env);
  return { contractsPath, worktreePath: run.worktreePath, publishEnv };
}

export async function runBuiltinIssue(issueNumber, {
  cwd = process.cwd(),
  repoRoot = rosterRoot,
  config = loadConfig({ repoRoot }),
  env = process.env,
  publish = false,
  seats = 'planner,coder',
  log = console.log,
  runCommand,
  fetchImpl,
  vault,
  runTestCommand,
  publisher = execFileAsync,
  now,
} = {}) {
  if (seats !== 'planner,coder') {
    throw new TypeError('Builtin seats must be planner,coder in that order');
  }
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
    sessionId: `roster-${issueNumber}-coder`, recordPreparation: false,
  });
  const { worktreePath } = prepared;
  const sessions = {
    planner: `roster-${prepared.issue.number}-planner`,
    coder: prepared.session,
  };
  const planner = await runPlanner({
    worktree: worktreePath, issue: prepared.issue, config, fetchImpl, env, vault,
  });
  const metricEnv = { ...commandEnv };
  for (const name of [...runNames, 'GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH',
    'GH_TOKEN', 'GITHUB_TOKEN']) delete metricEnv[name];
  const taskClass = inferTaskClass(prepared.issue.title);
  const recordSeat = async (session, run) => recordRun({
    session, task: prepared.task, task_class: taskClass,
  }, { cwd: prepared.repoRoot, env: { ...metricEnv, ...run.env } });
  const plannerRun = buildRun({
    config, usage: planner.usage ?? {}, session: sessions.planner, task: prepared.task,
    includeStub: true,
  });
  await recordSeat(sessions.planner, plannerRun);
  const result = await runCoder({
    worktree: worktreePath, repoRoot, config, task: prepared.task, session: sessions.coder,
    fetchImpl, env, vault, runTestCommand,
  });
  await ensureUnchanged(planner.recipePath, planner.recipe);
  await ensureUnchanged(planner.taskPath, planner.task);
  const coderRun = buildRun({
    config, usage: result.usage ?? {}, session: sessions.coder, task: prepared.task,
    includeStub: true,
  });
  await recordSeat(sessions.coder, coderRun);
  const runs = { planner: plannerRun, coder: coderRun };
  const command = `node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "feat: issue ${prepared.issue.number}" --merge-when-green`;
  log(`Worktree: ${worktreePath}\nAssignment: ${prepared.assignmentPath}\n` +
    `RECIPE: ${planner.recipePath}\nTASK: ${planner.taskPath}\nRESULT: ${result.resultPath}\n` +
    `Planner session: ${sessions.planner}\nAI-Run: ${plannerRun.line}\n` +
    `Coder session: ${sessions.coder}\nAI-Run: ${coderRun.line}\n` +
    `From the worktree root, publish only after reviewing changes:\n${command}`);

  const completed = {
    ...prepared, recipePath: planner.recipePath, taskPath: planner.taskPath,
    planner, result, sessions, runs, run: coderRun, command,
  };
  if (publish) {
    const { contractsPath, publishEnv } = await prepareBuiltinPublication(completed, {
      cwd, config, env,
    });
    const { stdout } = await publisher(process.execPath,
      [path.join(contractsPath, 'scripts', 'agent-pr.mjs'),
        '--message', `feat: issue ${prepared.issue.number}`, '--merge-when-green'],
      { cwd: worktreePath, env: publishEnv, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 });
    if (stdout?.trim()) log(stdout.trim());
  }
  return completed;
}
