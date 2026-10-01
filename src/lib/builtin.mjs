import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { buildPublishEnv, buildRun, resolvePublishModel, RUN_ENV_NAMES } from '../metrics/run.mjs';
import { taskFilesAllowed } from '../planner/stub.mjs';
import { runCoder } from '../seats/coder.mjs';
import { preparePlannerHandoff, readPlannerHandoff, runPlanner } from '../seats/planner.mjs';
import { requirePassingReview, runReviewer } from '../seats/reviewer.mjs';
import { isAllowedFile, isForbiddenWrite, isManagedFile } from '../runtime/tools.mjs';
import { checkExcellence, redactEvidence } from '../runtime/excellence.mjs';
import { isReviewRequired, loadConfig, requirePublicationEnabled, withoutLlmKeys } from './config.mjs';
import { loadFleet, withFleetProfile } from './fleet.mjs';
import { runIssue, validateIssueNumber } from './issue.mjs';
import {
  commentMergedIssue, mergedPullNumber, mergedPullNumberFromFailure,
} from './issue-board.mjs';
import { IDENTIFIER, inferTaskClass, recordRun } from './learn.mjs';
import { loadMetrics } from './metrics.mjs';
import { ensureLocalPath, resolveContractsPath } from './paths.mjs';
import { buildPublishMessage, formatPublishCommand, formatPublishEnvironment } from './publication.mjs';
import { archiveRunArtifacts } from './run-artifacts.mjs';
import { createRunLog } from './run-log.mjs';
import { formatRoute, routeTask } from './route.mjs';

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
  config = loadConfig({ repoRoot: rosterRoot, cwd }),
  env = process.env,
  skipReview = false,
} = {}) {
  requirePublicationEnabled(config);
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
  await requirePassingReview(run, skipReview || !isReviewRequired(config));
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
      provider: run.runs.coder.provider,
      ...excellenceFields(excellence, { config, env }),
    }, {
      cwd: run.repoRoot ?? run.worktreePath, env: run.runs.coder.env, run: run.runs.coder,
      createDirectory: Boolean(run.repoRoot),
    });
    throw new Error(`Publishing refused by excellence gate: ${excellence.reasons[0]}`);
  }
  const commandEnv = withoutLlmKeys(env, config);
  await git(run.worktreePath, ['submodule', 'update', '--init', '--recursive'], commandEnv);
  const contractsPath = resolveContractsPath({ repoRoot: run.worktreePath, cwd, env });
  await stageReviewedFiles(run.worktreePath, taskFilesAllowed(run.planner.task), { env: commandEnv });
  publishEnv.GITHUB_APP_PRIVATE_KEY_PATH = path.resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH);
  return { contractsPath, worktreePath: run.worktreePath, publishEnv, model: publishEnv.AI_MODEL };
}

export async function runBuiltinTask({
  cwd = process.cwd(), repoRoot = rosterRoot, config = loadConfig({ repoRoot, cwd }), env = process.env,
  task = env.AI_TASK || `local-${randomBytes(8).toString('hex')}`,
  session = env.AI_SESSION || `roster-${randomBytes(8).toString('hex')}-coder`,
  log = console.log, errorOutput = process.stderr, fetchImpl, vault, runTestCommand,
} = {}) {
  if (typeof task !== 'string' || !IDENTIFIER.test(task) ||
      typeof session !== 'string' || !IDENTIFIER.test(session)) {
    throw new TypeError('AI_TASK and AI_SESSION must be opaque 1-64 character identifiers');
  }
  resolveContractsPath({ repoRoot, cwd, env });
  const worktreePath = path.resolve(cwd);
  const reviewerSession = `roster-${randomBytes(8).toString('hex')}-reviewer`;
  const existingRuns = await fs.readdir(path.join(worktreePath, '.roster', 'runs')).catch((error) => {
    if (error.code === 'ENOENT') return null;
    throw error;
  });
  const journalEnabled = existingRuns !== null &&
    (!existingRuns.length || existingRuns.some((file) => file.endsWith('.jsonl')));
  const liveLog = await createRunLog({
    repoRoot: worktreePath, session, env, apiKeyEnv: config.llm.api_key_env, errorOutput,
  });
  const metricEnv = { ...env };
  for (const name of [...RUN_ENV_NAMES, config.llm.api_key_env,
    'GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH', 'GH_TOKEN', 'GITHUB_TOKEN']) delete metricEnv[name];
  const record = async (result) => {
    if (!journalEnabled) return;
    await recordRun({
      task, session, task_class: result.taskMetadata?.task_class,
      provider: result.run?.provider,
      ...excellenceFields(result.excellence, { config, env }),
    }, { cwd: worktreePath, env: { ...metricEnv, ...result.run?.env }, run: result.run });
  };
  const reviewSeat = async (result) => {
    const reviewConfig = { ...config, llm: { ...config.llm,
      model: result.mode === 'llm' ? result.model : config.llm.model } };
    const review = await liveLog.seat('reviewer', reviewerSession, reviewConfig, (onEvent) => runReviewer({
      worktree: worktreePath, repoRoot, config: reviewConfig, coderResult: result,
      env, fetchImpl, vault, onEvent,
    }));
    const reviewRun = review.queried ? buildRun({
      config: reviewConfig, response: review.response, task, session: reviewerSession, env,
    }) : null;
    if (journalEnabled) await recordRun({ task, session: reviewerSession, provider: reviewRun?.provider }, {
      cwd: worktreePath, env: { ...metricEnv, ...reviewRun?.env }, run: reviewRun,
    });
    return { review, reviewRun };
  };
  let result;
  try {
    result = await liveLog.seat('coder', session, config, (onEvent) => runCoder({
      worktree: worktreePath, repoRoot, config, env, task, session,
      fetchImpl, vault, runTestCommand, onEvent,
    }));
  } catch (error) {
    if (error instanceof Error && error.result) {
      await record(error.result);
      error.result.review = (await reviewSeat(error.result)).review;
    }
    throw error;
  }
  await record(result);
  const { review, reviewRun } = await reviewSeat(result);
  log(`Worktree: ${worktreePath}\nTASK: ${path.join(worktreePath, 'TASK.md')}\n` +
    `Live log: ${liveLog.path}\n` +
    `CONTEXT: ${result.contextPath}\nRESEARCH: ${result.researchPath}\nRESULT: ${result.resultPath}\n` +
    `REVIEW: ${review.reviewPath} (${review.verdict})\n` +
    `Mode: ${result.mode}\n` + (result.run ? `AI-Run: ${result.run.line}\n` : '') +
    (reviewRun ? `Reviewer AI-Run: ${reviewRun.line}\n` : '') +
    'Single coder task complete; publication remains an explicit reviewed App SDK handoff.');
  return { worktreePath, task, session, reviewerSession, result, review, run: result.run,
    reviewerRun: reviewRun, logPath: liveLog.path, logSession: liveLog.session };
}

export async function runBuiltinIssue(issueNumber, {
  cwd = process.cwd(),
  repoRoot = rosterRoot,
  config = loadConfig({ repoRoot, cwd }),
  env = process.env,
  publish = false,
  seats = 'planner,coder,reviewer',
  autoModel = false,
  skipReview = false,
  log = console.log,
  errorOutput = process.stderr,
  runCommand,
  fetchImpl,
  vault,
  runTestCommand,
  publisher = execFileAsync,
  issueCommenter = commentMergedIssue,
  metricsLoader = loadMetrics,
  now,
} = {}) {
  validateIssueNumber(issueNumber);
  if (!autoModel && !config.llm.model && (env.AI_MODEL || env.ROSTER_MODEL)) {
    config = { ...config, llm: Object.freeze({ ...config.llm, model: resolvePublishModel({ config, env }) }) };
  }
  if (!['planner,coder', 'planner,coder,reviewer'].includes(seats)) {
    throw new TypeError('Builtin seats must be planner,coder,reviewer in that order');
  }
  if (typeof autoModel !== 'boolean') throw new TypeError('--auto-model must be a boolean');
  if (typeof skipReview !== 'boolean') throw new TypeError('--skip-review must be a boolean');
  if (publish) requirePublicationEnabled(config);
  const reviewBypass = skipReview || !isReviewRequired(config);
  const fleet = autoModel ? await loadFleet({ cwd }) : null;
  if (autoModel && !fleet.profiles.length) {
    throw new Error('--auto-model requires at least one registered fleet profile; run roster onboard or fleet add');
  }
  if (!autoModel && config.llm.base_url && !config.llm.model) {
    throw new Error('set model: Set config.llm.model, AI_MODEL, or ROSTER_MODEL, or use --auto-model');
  }
  if (publish && !autoModel) resolvePublishModel({ config, env });
  if (!autoModel && config.llm.base_url && config.llm.model) buildRun({ config, response: null, env });
  if (publish && ((!autoModel && !config.llm.base_url) || !env.GITHUB_APP_ID || !env.GITHUB_APP_PRIVATE_KEY_PATH)) {
    throw new Error('--publish requires an LLM endpoint and GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH');
  }
  const commandEnv = withoutLlmKeys(env, config);
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
  let route = null;
  if (autoModel) {
    const taskClass = prepared.metadata?.task_class ?? inferTaskClass(prepared.issue.title);
    if (taskClass) {
      route = await routeTask({
        cwd: prepared.repoRoot, installationRoot: repoRoot, fleet,
        taskClass, difficulty: prepared.metadata?.difficulty ?? 2,
        records: metricsLoader({ contractsPath, cwd: prepared.repoRoot }),
      });
    }
    if (route) {
      autoRecommendation = route.recommendation;
      const selected = withFleetProfile(config, route.profile);
      activeConfig = { ...selected, llm: Object.freeze({
        ...selected.llm, effort: autoRecommendation?.effort ?? config.llm.effort,
      }) };
      log(`Auto-model: ${formatRoute(route, taskClass).trimEnd()}`);
    } else {
      activeConfig = { ...config, llm: Object.freeze({ ...config.llm, base_url: '', model: '' }) };
      log(`Auto-model: ${taskClass ? 'no eligible fleet profile or evidence' : 'no recognized task class'}; deterministic stub`);
    }
  }
  const existing = prepared.reused ? await readPlannerHandoff({
    worktree: worktreePath, reference: `issue:${prepared.issue.number}`, ask: prepared.ask, lockedModel: route?.profile.model,
    issueTitle: prepared.issue.title, issueBody: prepared.issue.body,
  }) : { plan: null };
  if (existing.reason) log(existing.reason);
  const archivePath = prepared.reused ? await archiveRunArtifacts(worktreePath, {
    task: prepared.task, git: (args) => git(worktreePath, args, commandEnv),
    preserve: existing.plan ? ['RECIPE.yml', 'TASK.md'] : [],
  }) : null;
  if (archivePath) log(`Previous generated run artifacts preserved: ${archivePath}`);
  const sessions = {
    planner: `roster-${prepared.issue.number}-planner`,
    coder: prepared.session,
    reviewer: `roster-${prepared.issue.number}-reviewer`,
  };
  const liveLog = await createRunLog({
    repoRoot: prepared.repoRoot, session: prepared.session, env,
    apiKeyEnv: activeConfig.llm.api_key_env, errorOutput, now,
  });
  const metricEnv = { ...commandEnv };
  for (const name of [...RUN_ENV_NAMES, 'GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH',
    'GH_TOKEN', 'GITHUB_TOKEN']) delete metricEnv[name];
  let planner;
  try {
    planner = existing.plan ? await preparePlannerHandoff(existing.plan, {
      worktree: worktreePath, learningRoot: prepared.repoRoot, config: activeConfig, env,
    }) : await liveLog.seat('planner', sessions.planner, activeConfig, (onEvent) => runPlanner({
      worktree: worktreePath, repoRoot, issue: prepared.issue, config: activeConfig,
      ask: prepared.ask, metadata: prepared.metadata ?? undefined, task: prepared.task,
      session: sessions.planner, fetchImpl, env, vault, learningRoot: prepared.repoRoot,
      lockedModel: route?.profile.model, onEvent,
    }));
    if (planner.reused) log('Planner skipped: existing RECIPE.yml and TASK.md validate; starting coder.');
  } catch (error) {
    if (error instanceof Error && error.run) {
      await recordRun({ session: sessions.planner, task: prepared.task,
        task_class: prepared.metadata?.task_class ?? inferTaskClass(prepared.issue.title) }, {
        cwd: prepared.repoRoot, env: { ...metricEnv, ...error.run.env }, createDirectory: true, run: error.run,
      });
    }
    throw error;
  }
  const taskClass = planner.metadata.task_class;
  const recordSeat = async (session, run, excellence) => recordRun({
    session, task: prepared.task, task_class: taskClass, provider: run?.provider,
    ...(excellence ? excellenceFields(excellence, { config, env }) : {}),
  }, { cwd: prepared.repoRoot, env: { ...metricEnv, ...run?.env }, createDirectory: true, run });
  const plannerRun = planner.run;
  if (!planner.reused) await recordSeat(sessions.planner, plannerRun);
  if (planner.error) log(`Planning failed: ${planner.error}\nRECIPE/TASK stubs are unverified; no configured coder will run.`);
  const coderConfig = { ...activeConfig, llm: Object.freeze({
    ...activeConfig.llm, model: planner.metadata.model || activeConfig.llm.model,
    ...(planner.error ? { base_url: '' } : {}),
    effort: planner.feedback?.effort ?? activeConfig.llm.effort,
  }) };
  const reviewSeat = async (coderResult) => {
    const reviewConfig = { ...coderConfig, llm: {
      ...coderConfig.llm, model: coderResult.mode === 'llm' ? coderResult.model : coderConfig.llm.model,
    } };
    const review = await liveLog.seat('reviewer', sessions.reviewer, reviewConfig, (onEvent) => runReviewer({
      worktree: worktreePath, repoRoot, config: reviewConfig,
      coderResult, fetchImpl, env, vault, onEvent,
    }));
    const reviewerRun = review.queried ? buildRun({
      config: reviewConfig, response: review.response, session: sessions.reviewer,
      task: prepared.task, env,
    }) : null;
    await recordSeat(sessions.reviewer, reviewerRun);
    return { review, reviewerRun };
  };
  let result;
  try {
    result = await liveLog.seat('coder', sessions.coder, coderConfig, (onEvent) => runCoder({
      worktree: worktreePath, repoRoot, config: coderConfig, task: prepared.task, session: sessions.coder,
      fetchImpl, env, vault, runTestCommand, priorFeedback: planner.feedback?.context, onEvent,
    }));
  } catch (error) {
    if (error instanceof Error && error.result) {
      await recordSeat(sessions.coder, error.result.run, error.result.excellence);
      error.result.review = (await reviewSeat(error.result)).review;
    }
    throw error;
  }
  const coderRun = result.run;
  await recordSeat(sessions.coder, coderRun, result.excellence);
  const { review, reviewerRun } = await reviewSeat(result);
  await ensureUnchanged(planner.recipePath, planner.recipe);
  await ensureUnchanged(planner.taskPath, planner.task);
  await ensureUnchanged(planner.estimatePath, planner.estimate);
  const runs = { planner: plannerRun, coder: coderRun, reviewer: reviewerRun };
  const model = result.mode === 'llm' ? coderRun.metrics.model : null;
  const publishMessage = model ? buildPublishMessage({
    subject: `feat: issue ${prepared.issue.number}`, model, issueNumber: prepared.issue.number,
    summary: redactEvidence(result.summary, { env, apiKeyEnv: config.llm.api_key_env }),
    testsSkipped: result.testsSkipped,
    seats: `planner, coder, reviewer (${skipReview ? 'gate bypassed with --skip-review'
      : !isReviewRequired(config) ? 'gate not required by configuration' : review.verdict})`,
  }) : null;
  const command = model && config.publish?.enabled !== false && (review.verdict === 'pass' || reviewBypass)
    ? formatPublishCommand({ message: publishMessage, model }) : null;
  log(`Worktree: ${worktreePath}\nAssignment: ${prepared.assignmentPath}\n` +
    `Live log: ${liveLog.path}\n` +
    `RECIPE: ${planner.recipePath}\nTASK: ${planner.taskPath}\nESTIMATE: ${planner.estimatePath}\n` +
    `RESULT: ${result.resultPath}\nREVIEW: ${review.reviewPath} (${review.verdict})\n` +
    `Planner session: ${sessions.planner}\n` +
    (plannerRun ? `AI-Run: ${plannerRun.line}\n` : '') +
    `Coder session: ${sessions.coder}\n` +
    (coderRun ? `AI-Run: ${coderRun.line}\n` +
      `For manual publication, set these variables (empty values clear inherited fields):\n` +
      formatPublishEnvironment(coderRun.env)
      : 'Stub run: no AI-Run metadata and no code to publish.\n') +
    `Reviewer session: ${sessions.reviewer}\n` +
    (reviewerRun ? `AI-Run: ${reviewerRun.line}\n` : '') +
    (command ? `From the worktree root, publish only after reviewing changes:\n${command}`
      : config.publish?.enabled === false
        ? 'Publication unavailable: publishing is disabled by publish.enabled.'
        : model
        ? 'Publication unavailable: REVIEW.md failed; rerun the reviewer or explicitly use --skip-review.'
        : 'Publication unavailable: set model and complete a configured coder run with passing checks.'));

  const completed = {
    ...prepared, recipePath: planner.recipePath, taskPath: planner.taskPath,
    planner, result, review, sessions, runs, run: coderRun, command, autoRecommendation, route, archivePath,
    failed: Boolean(planner.error),
    logPath: liveLog.path, logSession: liveLog.session,
  };
  if (publish && planner.error) log('Publication skipped: planning failed; inspect the stub and rerun the issue.');
  if (publish && !planner.error) {
    const { contractsPath, publishEnv, model: publishModel } = await prepareBuiltinPublication(completed, {
      cwd, config: coderConfig, env, skipReview,
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
        await issueCommenter({
          issue: prepared.issue, pullNumber: merged, model: coderRun.env.AI_MODEL,
          runLine: coderRun.line, run: coderRun,
          repoRoot: prepared.repoRoot, cwd, env,
        });
      } catch (commentError) {
        throw new Error(`PR #${merged} merged, but issue comment and local cleanup failed: ${commentError.message}`, {
          cause: commentError,
        });
      }
      throw new Error(`PR #${merged} merged and issue commented, but local publisher cleanup failed; inspect the worktree`, {
        cause: error,
      });
    }
    if (stdout?.trim()) log(stdout.trim());
    const pullNumber = mergedPullNumber(stdout);
    await issueCommenter({
      issue: prepared.issue, pullNumber, model: coderRun.env.AI_MODEL,
      runLine: coderRun.line, run: coderRun,
      repoRoot: prepared.repoRoot, cwd, env,
    });
  }
  return completed;
}
