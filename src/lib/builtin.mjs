import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { buildPublishEnv, buildRun, resolvePublishModel, RUN_ENV_NAMES } from '../metrics/run.mjs';
import { taskFilesAllowed } from '../planner/stub.mjs';
import { runCoder } from '../seats/coder.mjs';
import { runPlanner } from '../seats/planner.mjs';
import { isAllowedFile, isForbiddenWrite, isManagedFile } from '../runtime/tools.mjs';
import { checkExcellence, redactEvidence } from '../runtime/excellence.mjs';
import { loadConfig } from './config.mjs';
import { runIssue } from './issue.mjs';
import {
  closeMergedIssue, mergedPullNumber, mergedPullNumberFromFailure,
} from './issue-board.mjs';
import { IDENTIFIER, inferTaskClass, recommend, recordRun } from './learn.mjs';
import { loadMetrics } from './metrics.mjs';
import { ensureLocalPath, resolveContractsPath } from './paths.mjs';
import { buildPublishMessage, formatPublishCommand } from './publication.mjs';

const execFileAsync = promisify(execFile);
const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));

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

function excellenceFields(excellence, { config, env }) {
  return {
    excellence: excellence.pass ? 'pass' : 'fail',
    defects: excellence.reasons.map((reason) => redactEvidence(reason, {
      env, apiKeyEnv: config.llm.api_key_env,
    })),
  };
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
    if (isManagedFile(file)) continue;
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
  if (run.result?.mode !== 'llm' || (run.result.tests?.exit_code !== 0 && !run.result.testsSkipped)) {
    throw new Error('Publishing requires a configured coder run with passing tests');
  }
  if (!run.result.excellence?.pass) throw new Error('Publishing requires a passing excellence gate');
  if (!env.GITHUB_APP_ID || !env.GITHUB_APP_PRIVATE_KEY_PATH) {
    throw new Error('Publishing requires GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH');
  }
  const publishEnv = buildPublishEnv({ config, env, run: run.runs.coder });
  await ensureUnchanged(run.recipePath, run.planner.recipe);
  await ensureUnchanged(run.taskPath, run.planner.task);
  await ensureUnchanged(run.planner.estimatePath, run.planner.estimate);
  const excellence = await checkExcellence({
    worktree: run.worktreePath, task: run.planner.task, result: run.result, baseline: run.result.baseline,
    verifiedSnapshot: run.result.excellence.snapshot,
    memoryPath: run.result.memoryPath,
    env, apiKeyEnv: config.llm.api_key_env,
  });
  if (!excellence.pass) {
    await recordRun({
      task: run.task, session: run.runs.coder.env.AI_SESSION,
      task_class: run.planner.metadata?.task_class,
      ...excellenceFields(excellence, { config, env }),
    }, {
      cwd: run.repoRoot ?? run.worktreePath, env: run.runs.coder.env,
      createDirectory: Boolean(run.repoRoot),
    });
    throw new Error(`Publishing refused by excellence gate: ${excellence.reasons[0]}`);
  }
  const commandEnv = { ...env };
  delete commandEnv[config.llm.api_key_env];
  await git(run.worktreePath, ['submodule', 'update', '--init', '--recursive'], commandEnv);
  const contractsPath = resolveContractsPath({ repoRoot: run.worktreePath, cwd, env });
  await stageReviewedFiles(run.worktreePath, taskFilesAllowed(run.planner.task), { env: commandEnv });
  publishEnv.GITHUB_APP_PRIVATE_KEY_PATH = path.resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH);
  return { contractsPath, worktreePath: run.worktreePath, publishEnv, model: publishEnv.AI_MODEL };
}

export async function runBuiltinTask({
  cwd = process.cwd(), repoRoot = rosterRoot, config = loadConfig({ repoRoot }), env = process.env,
  task = env.AI_TASK || `local-${randomBytes(8).toString('hex')}`,
  session = env.AI_SESSION || `roster-${randomBytes(8).toString('hex')}-coder`,
  log = console.log, fetchImpl, vault, runTestCommand,
} = {}) {
  if (typeof task !== 'string' || !IDENTIFIER.test(task) ||
      typeof session !== 'string' || !IDENTIFIER.test(session)) {
    throw new TypeError('AI_TASK and AI_SESSION must be opaque 1-64 character identifiers');
  }
  resolveContractsPath({ repoRoot, cwd, env });
  const worktreePath = path.resolve(cwd);
  const record = async (result) => {
    const metricEnv = { ...env };
    for (const name of [...RUN_ENV_NAMES, config.llm.api_key_env,
      'GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH', 'GH_TOKEN', 'GITHUB_TOKEN']) delete metricEnv[name];
    await recordRun({
      task, session, task_class: result.taskMetadata?.task_class,
      ...excellenceFields(result.excellence, { config, env }),
    }, { cwd: worktreePath, env: { ...metricEnv, ...result.run?.env } });
  };
  let result;
  try {
    result = await runCoder({ worktree: worktreePath, repoRoot, config, env, task, session,
      fetchImpl, vault, runTestCommand });
  } catch (error) {
    if (error instanceof Error && error.result) await record(error.result);
    throw error;
  }
  await record(result);
  log(`Worktree: ${worktreePath}\nTASK: ${path.join(worktreePath, 'TASK.md')}\n` +
    `CONTEXT: ${result.contextPath}\nRESEARCH: ${result.researchPath}\nRESULT: ${result.resultPath}\n` +
    `Mode: ${result.mode}\n` + (result.run ? `AI-Run: ${result.run.line}\n` : '') +
    'Single coder task complete; publication remains an explicit reviewed App SDK handoff.');
  return { worktreePath, task, session, result, run: result.run };
}

export async function runBuiltinIssue(issueNumber, {
  cwd = process.cwd(),
  repoRoot = rosterRoot,
  config = loadConfig({ repoRoot }),
  env = process.env,
  publish = false,
  seats = 'planner,coder',
  autoModel = false,
  log = console.log,
  runCommand,
  fetchImpl,
  vault,
  runTestCommand,
  publisher = execFileAsync,
  issueCloser = closeMergedIssue,
  metricsLoader = loadMetrics,
  now,
} = {}) {
  if (!config.llm.model && (env.AI_MODEL || env.ROSTER_MODEL)) {
    config = { ...config, llm: Object.freeze({ ...config.llm, model: resolvePublishModel({ config, env }) }) };
  }
  if (seats !== 'planner,coder') {
    throw new TypeError('Builtin seats must be planner,coder in that order');
  }
  if (typeof autoModel !== 'boolean') throw new TypeError('--auto-model must be a boolean');
  if (autoModel && config.llm.model) {
    throw new Error('--auto-model requires an empty config.llm.model');
  }
  if (!autoModel && config.llm.base_url && !config.llm.model) {
    throw new Error('set model: Set config.llm.model, AI_MODEL, or ROSTER_MODEL, or use --auto-model');
  }
  if (publish && !autoModel) resolvePublishModel({ config, env });
  if (config.llm.base_url && config.llm.model) buildRun({ config, env });
  if (publish && (!config.llm.base_url || !env.GITHUB_APP_ID || !env.GITHUB_APP_PRIVATE_KEY_PATH)) {
    throw new Error('--publish requires an LLM endpoint and GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH');
  }
  const commandEnv = { ...env };
  delete commandEnv[config.llm.api_key_env];
  const contractsPath = resolveContractsPath({ repoRoot, cwd, env });
  const issueCommand = runCommand ?? (async (program, args, workingDirectory) =>
    (await execFileAsync(program, args, { cwd: workingDirectory, env: commandEnv, encoding: 'utf8' })).stdout);
  const prepared = await runIssue(issueNumber, {
    cwd, runCommand: issueCommand, worktrees: config.paths.worktrees, log: () => {}, now, config,
    beforeWorktree: (root, worktreePath) => ensureLocalPath(worktreePath, root),
    sessionId: `roster-${issueNumber}-coder`, recordPreparation: false,
  });
  const { worktreePath } = prepared;
  let activeConfig = config;
  let autoRecommendation = null;
  if (autoModel) {
    const taskClass = inferTaskClass(prepared.issue.title);
    if (config.llm.base_url && taskClass) {
      autoRecommendation = recommend(metricsLoader({
        contractsPath, cwd: prepared.repoRoot,
      }), taskClass);
    }
    if (autoRecommendation) {
      if (!/^[A-Za-z0-9._:/-]+$/.test(autoRecommendation.model)) {
        throw new Error('The recommended model name is invalid');
      }
      activeConfig = { ...config, llm: Object.freeze({
        ...config.llm, model: autoRecommendation.model,
        effort: autoRecommendation.effort ?? config.llm.effort,
      }) };
      log(`Auto-model: ${autoRecommendation.model} from ${autoRecommendation.n} human evaluations`);
    } else {
      activeConfig = { ...config, llm: Object.freeze({ ...config.llm, base_url: '', model: '' }) };
      log(`Auto-model: ${config.llm.base_url && taskClass
        ? 'insufficient evaluated data' : 'no configured endpoint or task class'}; deterministic stub`);
    }
  }
  const sessions = {
    planner: `roster-${prepared.issue.number}-planner`,
    coder: prepared.session,
  };
  const planner = await runPlanner({
    worktree: worktreePath, repoRoot, issue: prepared.issue, config: activeConfig,
    task: prepared.task, session: sessions.planner, fetchImpl, env, vault, learningRoot: prepared.repoRoot,
  });
  const metricEnv = { ...commandEnv };
  for (const name of [...RUN_ENV_NAMES, 'GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH',
    'GH_TOKEN', 'GITHUB_TOKEN']) delete metricEnv[name];
  const taskClass = planner.metadata.task_class;
  const recordSeat = async (session, run, excellence) => recordRun({
    session, task: prepared.task, task_class: taskClass,
    ...(excellence ? excellenceFields(excellence, { config, env }) : {}),
  }, { cwd: prepared.repoRoot, env: { ...metricEnv, ...run?.env }, createDirectory: true });
  const plannerRun = activeConfig.llm.base_url ? buildRun({
    config: activeConfig, usage: planner.usage ?? {}, session: sessions.planner, task: prepared.task,
    env,
  }) : null;
  await recordSeat(sessions.planner, plannerRun);
  const coderConfig = { ...activeConfig, llm: Object.freeze({
    ...activeConfig.llm, model: planner.metadata.model || activeConfig.llm.model,
    effort: planner.feedback?.effort ?? activeConfig.llm.effort,
  }) };
  let result;
  try {
    result = await runCoder({
      worktree: worktreePath, repoRoot, config: coderConfig, task: prepared.task, session: sessions.coder,
      fetchImpl, env, vault, runTestCommand, priorFeedback: planner.feedback?.context,
    });
  } catch (error) {
    if (error instanceof Error && error.result) await recordSeat(sessions.coder, error.result.run, error.result.excellence);
    throw error;
  }
  await ensureUnchanged(planner.recipePath, planner.recipe);
  await ensureUnchanged(planner.taskPath, planner.task);
  await ensureUnchanged(planner.estimatePath, planner.estimate);
  const coderRun = result.mode === 'llm' ? buildRun({
    config: coderConfig, usage: result.usage ?? {}, session: sessions.coder, task: prepared.task,
    env,
  }) : null;
  await recordSeat(sessions.coder, coderRun, result.excellence);
  const runs = { planner: plannerRun, coder: coderRun };
  const model = result.mode === 'llm' ? resolvePublishModel({ config: coderConfig, env }) : null;
  const publishMessage = model ? buildPublishMessage({
    subject: `feat: issue ${prepared.issue.number}`, model, issueNumber: prepared.issue.number,
    summary: redactEvidence(result.summary, { env, apiKeyEnv: config.llm.api_key_env }),
    testsSkipped: result.testsSkipped,
  }) : null;
  const command = model ? formatPublishCommand({ message: publishMessage, model }) : null;
  log(`Worktree: ${worktreePath}\nAssignment: ${prepared.assignmentPath}\n` +
    `RECIPE: ${planner.recipePath}\nTASK: ${planner.taskPath}\nESTIMATE: ${planner.estimatePath}\nRESULT: ${result.resultPath}\n` +
    `Planner session: ${sessions.planner}\n` +
    (plannerRun ? `AI-Run: ${plannerRun.line}\n` : '') +
    `Coder session: ${sessions.coder}\n` +
    (coderRun ? `AI-Run: ${coderRun.line}\n` +
      `For manual publication, set these environment variables:\n` +
      Object.entries(coderRun.env).map(([name, value]) => `${name}=${value}\n`).join('')
      : 'Stub run: no AI-Run metadata and no code to publish.\n') +
    (command ? `From the worktree root, publish only after reviewing changes:\n${command}`
      : 'Publication unavailable: set model and complete a configured coder run with passing checks.'));

  const completed = {
    ...prepared, recipePath: planner.recipePath, taskPath: planner.taskPath,
    planner, result, sessions, runs, run: coderRun, command, autoRecommendation,
  };
  if (publish) {
    const { contractsPath, publishEnv, model: publishModel } = await prepareBuiltinPublication(completed, {
      cwd, config: coderConfig, env,
    });
    let stdout;
    try {
      ({ stdout } = await publisher(process.execPath,
        [path.join(contractsPath, 'scripts', 'agent-pr.mjs'),
          '--message', publishMessage, '--model', publishModel, '--merge-when-green'],
        { cwd: worktreePath, env: publishEnv, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 }));
    } catch (error) {
      if (String(error.stderr ?? error.message).includes('HTTP 422')) {
        throw new Error('Checks permission is not accepted on the installation.', { cause: error });
      }
      const merged = mergedPullNumberFromFailure(error.stderr ?? error.message);
      if (merged === null) throw error;
      try {
        await issueCloser({
          issue: prepared.issue, pullNumber: merged, runLine: coderRun.line,
          repoRoot: prepared.repoRoot, cwd, env,
        });
      } catch (closeError) {
        throw new Error(`PR #${merged} merged, but issue closure and local cleanup failed: ${closeError.message}`, {
          cause: closeError,
        });
      }
      throw new Error(`PR #${merged} merged and issue closed, but local publisher cleanup failed; inspect the worktree`, {
        cause: error,
      });
    }
    if (stdout?.trim()) log(stdout.trim());
    const pullNumber = mergedPullNumber(stdout);
    await issueCloser({
      issue: prepared.issue, pullNumber, runLine: coderRun.line,
      repoRoot: prepared.repoRoot, cwd, env,
    });
  }
  return completed;
}
