import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { buildPublishEnv, buildRun, resolvePublishModel, RUN_ENV_NAMES } from '../metrics/run.mjs';
import { cleanAskText, renderAssignment, taskFilesAllowed } from '../planner/stub.mjs';
import { assertSeatCovers, parseRecipe } from './recipe.mjs';
import { taskSkillNames } from '../runtime/skills.mjs';
import { runCoder } from '../seats/coder.mjs';
import { acceptPlannerPlan, preparePlannerHandoff, readPlannerHandoff, readPlannerTask, readPreviousReview, runPlanner } from '../seats/planner.mjs';
import { requirePassingReview, runReviewer } from '../seats/reviewer.mjs';
import {
  isAllowedFile, isForbiddenWrite, isManagedFile, isRepairTestFile, isScopeExpansionFile, taskAndRepairFiles, ToolAccessError,
} from '../runtime/tools.mjs';
import { checkExcellence, redactEvidence } from '../runtime/excellence.mjs';
import { requireLifecycleHooks } from '../runtime/hooks.mjs';
import { isReviewRequired, loadConfig, requirePublicationEnabled, withoutLlmKeys } from './config.mjs';
import { loadFleet, withFleetProfile } from './fleet.mjs';
import { runIssue, validateIssueNumber } from './issue.mjs';
import {
  commentMergedIssue, mergedPullNumber, mergedPullNumberFromFailure, setIssueRunStatus,
} from './issue-board.mjs';
import { IDENTIFIER, inferTaskClass, loadLearning, recordDeliveryPublication, recordRun, selectedAttemptRecord } from './learn.mjs';
import { captureLifecycleEvent, provenanceOptOut, provenanceStoreForRun } from './local-runs.mjs';
import { loadMetrics } from './metrics.mjs';
import { ensureLocalPath, resolveContractsPath } from './paths.mjs';
import { buildPublishMessage, formatPublishCommand, formatPublishEnvironment } from './publication.mjs';
import { archivedRunScope, archiveRunArtifacts, latestArchivedReview } from './run-artifacts.mjs';
import { createRunLog } from './run-log.mjs';
import { humanEvalHint, recordedCoderRun } from './seat-publication.mjs';
import { formatRoute, routeTask } from './route.mjs';
import { classifyAsk, clarificationHint } from '../planner/classify.mjs';
import { parseTaskDocument } from '../planner/task.mjs';
import { retryCommandForTask } from '../llm/request.mjs';
import { selectReasoning } from '../llm/reasoning.mjs';
import { readTaskMetadata } from '../runtime/estimate.mjs';
import { loadCapabilities } from './capabilities.mjs';
import { initializeWorktreeSubmodules } from './contracts.mjs';
import { formatStart, resolveStart, startOptions } from './start.mjs';
import { createDebugLog } from './debug-log.mjs';
import { isRunCancelled, throwIfCancelled } from '../runtime/cancel.mjs';
import { earlierWaveFiles, issueWave, readWavePlan, requireEarlierWavesClosed, waveBoard } from './waves.mjs';
import { githubRepository } from './issue.mjs';
import { routeFailure } from '../llm/openai.mjs';
import { acquireRepoLock } from './repo-locks.mjs';
import { parallelLimit, runReadyWaves } from './wave-scheduler.mjs';
import { attemptLimit, attemptSummary, runPlanAttempts } from './attempts.mjs';

const execFileAsync = promisify(execFile);
const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));
export const maxPerspectiveEscalations = 2;
export const maxRescopes = 2;
export const maxReviewRepairs = 2;
const maxScopeExpansion = 16;
const claimHook = Symbol('issue claim after validation');

// A semantic reviewer fail is actionable feedback, not a terminal verdict: hand the findings back to the coder.
export function reviewRepairContinuation({ round, reasons, unmetChecks = [], stalled = false, changedFiles = [],
  heading = `Review repair ${round}: the reviewer failed the previous result` }) {
  return heading +
    (unmetChecks.length ? ` (unmet acceptance checks: ${unmetChecks.join(', ')})` : '') + '. Its edits remain in the ' +
    'worktree' + (changedFiles.length ? ` (${changedFiles.join(', ')})` : '') + '. ' +
    (stalled ? 'The same checks stayed unmet after the last repair, so do not repeat that approach: re-derive the ' +
      'required behavior from TASK.md and change strategy. ' : '') +
    'Address every finding below with the smallest correct change, write any missing required file in several ' +
    'smaller write/edit calls rather than one huge call, keep work that already meets its check, and rerun ' +
    'node --test until it exits 0. RESULT.md and REVIEW.md are harness-written and read-only to you; findings about ' +
    'missing test evidence are met by tests that pass, which the regenerated RESULT.md then shows. ' +
    'In your summary, map each acceptance check to the evidence that now meets it.' +
    '\n\nReviewer findings (redacted, truncated):\n' +
    reasons.map((reason) => `- ${reason}`).join('\n').slice(0, 3000);
}

// A rerun of the same TASK starts from the last failed review's findings instead of repeating the same attempt.
export function previousReviewFindings(review) {
  if (typeof review !== 'string' || !review.startsWith('# Review\n\nVerdict: fail\n')) return undefined;
  const section = (name) => (review.split(`\n## ${name}\n`)[1] ?? '').split('\n## ')[0];
  const reasons = section('Reasons').split('\n').filter((line) => line.startsWith('- ')).map((line) => line.slice(2));
  // A reviewer that could not complete gave no findings; an older complete review is better evidence.
  if (!reasons.length || reasons.every((reason) => reason.startsWith('Reviewer could not complete'))) return undefined;
  const unmetChecks = [...section('Acceptance checks').matchAll(/^- \[ \] (\d+)\. /gm)].map(([, id]) => Number(id));
  return { reasons, unmetChecks };
}

export function previousReviewContinuation(review) {
  const findings = previousReviewFindings(review);
  return findings && reviewRepairContinuation({ ...findings,
    heading: 'Previous run: the reviewer failed the last result for this same TASK' });
}

// Returns the raised expansion budget when the coder was blocked only by the scope limit, else null.
export function rescopeBudget(error, current, attempts) {
  const blocked = error?.result?.scopeBlocked;
  if (!Array.isArray(blocked) || !blocked.length || attempts >= maxRescopes) return null;
  if (!Number.isSafeInteger(current) || current <= 0 || current >= maxScopeExpansion) return null;
  return Math.min(maxScopeExpansion, Math.max(current * 2, current + blocked.length + 1));
}

export function rescopeContinuation({ previous, budget, files, changedFiles = [] }) {
  return `Re-scoped: the previous coder context reached its ${previous}-file expansion budget and needed ` +
    `${files.join(', ')}. The budget is now ${budget} files outside TASK.md Allowed Files. Its edits remain in the ` +
    'worktree' + (changedFiles.length ? ` (${changedFiles.join(', ')})` : '') + '. Re-read TASK.md and the current ' +
    'diff, write the files the change genuinely needs, justify each file outside the plan in your summary (the reviewer ' +
    'judges it), and rerun node --test until it exits 0.';
}

// Budget exhaustion means the coder's context is stuck; hard denials, cancellation, and setup errors are not.
export function coderStuckReason(error) {
  if (!(error instanceof Error) || !error.result) return null;
  const chain = [];
  for (let current = error; current instanceof Error && !chain.includes(current); current = current.cause) chain.push(current);
  if (chain.some((entry) => entry instanceof ToolAccessError && !/\(repeated after \d+ denials\)/.test(entry.message))) {
    return null;
  }
  const text = chain.map((entry) => entry.message).join('\n');
  if (error.result.contextHandoff === true || /Test repair handoff: context \d+ of \d+ tokens/.test(text)) {
    return 'filled half its context while repairs were still progressing';
  }
  if (error.result.repairRepeated === true || /Test repair stalled: an earlier failure repeated/.test(text)) {
    return 'repeated an earlier test failure';
  }
  if (error.result.substanceUnresolved === true) {
    return 'left a new test that does not exercise app code after its correction';
  }
  if (error.result.repairBudgetExhausted === true || /Test repair budget \(\d+\) exhausted/.test(text)) {
    return 'exhausted its test repair budget';
  }
  if (/Coder turn budget \(\d+\) exhausted/.test(text)) return 'exhausted its turn budget';
  if (/continued exploring after the bounded exploration budget/.test(text)) return 'kept exploring without converging';
  if (/\(repeated after \d+ denials\)/.test(text)) return 'repeated a denied action';
  return null;
}

export function perspectiveContinuation({ attempt, reason, evidence, changedFiles = [], history = [],
  sameModel = false }) {
  return `Fresh perspective ${attempt}: the previous coder context ${reason} and was stopped so it would not keep ` +
    'repeating the same moves. Its edits remain in the worktree' +
    (changedFiles.length ? ` (${changedFiles.join(', ')})` : '') + '. ' +
    (history.length > 1 ? 'Earlier contexts on this TASK: ' + history.map((entry, index) =>
      `${index + 1}) ${entry.model ?? 'unknown model'} ${entry.reason}` +
      (Number.isSafeInteger(entry.failCount) ? ` with ${entry.failCount} failing test(s)` : '')).join('; ') +
      '. Do not retry their approaches. ' : '') +
    (sameModel ? 'You run on the same model as the stopped context, so only a different approach can change the ' +
      'outcome. ' : '') +
    'Before editing, read the TASK checks, the ' +
    'current implementation, and the current tests. For each remaining failure, decide from TASK.md which side is ' +
    'wrong (the assertion or the implementation) and change only that side; never alternate between editing a test ' +
    'and its implementation to chase the same assertion. Keep correct work, prefer the simplest behavior that ' +
    'satisfies TASK.md, and rerun node --test until it exits 0.\n\nLast failure evidence (redacted, truncated):\n' +
    String(evidence ?? '').slice(0, 2000);
}

// A resumed run keeps only harness-recorded scope whose files are still changed in the worktree.
export async function restoreRunScope(worktree, task, runGit, allowedFiles = []) {
  const recorded = await archivedRunScope(worktree, { task, git: runGit });
  const changed = new Set([
    ...(await runGit(['diff', '--name-only', '-z', 'HEAD'])).split('\0'),
    ...(await runGit(['ls-files', '--others', '--exclude-standard', '-z'])).split('\0'),
  ].filter(Boolean).map((file) => file.replaceAll('\\', '/')));
  const keep = (file, valid) => changed.has(file) && valid(file) && !isAllowedFile(file, allowedFiles);
  return {
    repairFiles: recorded.repairFiles.filter((file) => keep(file, isRepairTestFile)),
    scopeFiles: recorded.scopeFiles.filter((file) => keep(file, isScopeExpansionFile)),
  };
}

async function git(worktree, args, env = process.env) {
  const { stdout } = await execFileAsync('git', args, {
    cwd: worktree, env, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024,
  });
  return stdout;
}

// A worktree whose Git registration was removed resolves to the parent checkout and lists no files,
// so seats would plan or edit without repository grounding; fail before any seat runs.
export async function requireWorktreeCheckout(worktree, env = process.env) {
  let top;
  try {
    top = (await git(worktree, ['rev-parse', '--show-toplevel'], env)).trim();
  } catch (error) {
    throw new Error(`Worktree ${worktree} is not a Git checkout; remove it and rerun.`, { cause: error });
  }
  const normalize = async (value) => {
    const real = await fs.realpath(value).catch(() => path.resolve(value));
    return process.platform === 'win32' ? real.toLowerCase() : real;
  };
  if (!top || await normalize(top) !== await normalize(worktree)) {
    throw new Error(`Worktree ${worktree} is not a registered Git worktree (Git resolves it to ${top || 'nothing'}); ` +
      'remove the directory and rerun so Roster can recreate it.');
  }
}

// Diff summary of reviewed files; `git diff HEAD` omits untracked files, so new files are listed too.
export async function reviewedDiffStat(worktree, files, env = process.env) {
  if (!files.length) return '';
  const changed = (await git(worktree, ['diff', '--stat', 'HEAD', '--', ...files], env)).trim();
  const added = (await git(worktree, ['ls-files', '--others', '--exclude-standard', '--', ...files], env))
    .split('\n').map((line) => line.trim()).filter(Boolean);
  return [changed, ...added.map((file) => ` ${file} | new file`)].filter(Boolean).join('\n');
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
  signal,
} = {}) {
  throwIfCancelled(signal);
  requirePublicationEnabled(config);
  if (run?.attempt && (run.attempt.winner !== run.attempt.index ||
      selectedAttemptRecord(loadLearning({ cwd: run.repoRoot ?? run.worktreePath }).runs, run.task)?.session !== run.session)) {
    throw new Error('Only the recorded winning attempt may be published; review bypass cannot select a loser');
  }
  if (run?.planningOnly || run?.askKind && run.askKind !== 'slice') {
    throw new Error('Planning-only output is not code to publish; create and run a bounded slice first');
  }
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
  buildPublishEnv({ config, env, run: run.runs.coder });
  const recorded = recordedCoderRun({ repoRoot: run.repoRoot ?? run.worktreePath, run: run.runs.coder });
  const publishEnv = buildPublishEnv({ config, env, run: recorded });
  await requirePassingReview(run, skipReview || !isReviewRequired(config));
  await ensureUnchanged(run.recipePath, run.planner.recipe);
  await ensureUnchanged(run.taskPath, run.planner.task);
  await ensureUnchanged(run.planner.estimatePath, run.planner.estimate);
  await requireLifecycleHooks('pre-publish', { worktree: run.worktreePath, env,
    apiKeyEnv: config.llm.api_key_env, memoryPath: run.result.memoryPath, signal });
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
  await initializeWorktreeSubmodules(run.worktreePath, async (_program, args, worktree) =>
    git(worktree, args, commandEnv));
  const contractsPath = resolveContractsPath({ repoRoot: run.worktreePath, cwd, env });
  await stageReviewedFiles(run.worktreePath, taskAndRepairFiles(taskFilesAllowed(run.planner.task),
    run.result.repairFiles, run.result.scopeFiles), { env: commandEnv });
  publishEnv.GITHUB_APP_PRIVATE_KEY_PATH = path.resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH);
  return { contractsPath, worktreePath: run.worktreePath, publishEnv, model: publishEnv.AI_MODEL };
}

export async function runBuiltinTask({
  cwd = process.cwd(), repoRoot = rosterRoot, config = loadConfig({ repoRoot, cwd }), env = process.env,
  task = env.AI_TASK || `local-${randomBytes(8).toString('hex')}`,
  session = env.AI_SESSION || `roster-${randomBytes(8).toString('hex')}-coder`,
  log = console.log, errorOutput = process.stderr, fetchImpl, vault, runTestCommand,
  debug = createDebugLog({ env }),
  onRunEvent,
  signal,
  steeringControl,
} = {}) {
  throwIfCancelled(signal);
  config = { ...config, capabilities: config.capabilities ?? await loadCapabilities({ cwd }) };
  if (typeof task !== 'string' || !IDENTIFIER.test(task) ||
      typeof session !== 'string' || !IDENTIFIER.test(session)) {
    throw new TypeError('AI_TASK and AI_SESSION must be opaque 1-64 character identifiers');
  }
  resolveContractsPath({ repoRoot, cwd, env });
  const worktreePath = path.resolve(cwd);
  const retryCommand = 'roster run --seat coder --runtime builtin';
  const taskSource = await readPlannerTask(worktreePath);
  if (taskSource === null) throw new Error('An existing TASK.md is required before executing a coder seat');
  const document = parseTaskDocument(taskSource);
  const classification = classifyAsk(document.ask, { title: document.title, filesAllowed: document.files_allowed });
  const askKind = classification.kind;
  const previousReview = await readPreviousReview(worktreePath);
  const previousCoder = previousReview?.startsWith('# Review\n\nVerdict: fail\n')
    ? loadLearning({ cwd: worktreePath }).runs.findLast((run) => run.task === task && run.excellence !== undefined)
    : null;
  if (previousCoder && ['l', 'm', 'h', 'x'].includes(previousCoder.effort)) {
    config = { ...config, llm: { ...config.llm, review_retry_effort: previousCoder.effort } };
  }
  config = selectReasoning(config, { kind: askKind, taskClass: readTaskMetadata(taskSource).task_class,
    difficulty: readTaskMetadata(taskSource).difficulty });
  log(`Ask kind: ${askKind} (${classification.reason})`);
  if (askKind === 'clarify') {
    log(clarificationHint);
    return { worktreePath, task, session, askKind, classification, planningOnly: true,
      clarification: clarificationHint, command: null, failed: false };
  }
  const reviewerSession = `roster-${randomBytes(8).toString('hex')}-reviewer`;
  const existingRuns = await fs.readdir(path.join(worktreePath, '.roster', 'runs')).catch((error) => {
    if (error.code === 'ENOENT') return null;
    throw error;
  });
  const journalEnabled = existingRuns !== null &&
    (!existingRuns.length || existingRuns.some((file) => file.endsWith('.jsonl')));
  const liveLog = await createRunLog({
    repoRoot: worktreePath, session, env, apiKeyEnv: config.llm.api_key_env, errorOutput,
    debug, issue: /^issue-([1-9]\d*)$/.test(task) && Number.isSafeInteger(Number(task.slice(6)))
      ? Number(task.slice(6)) : null,
    observe: onRunEvent,
  });
  const metricEnv = { ...env };
  for (const name of [...RUN_ENV_NAMES, config.llm.api_key_env,
    'GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH', 'GH_TOKEN', 'GITHUB_TOKEN']) delete metricEnv[name];
  if (askKind !== 'slice') {
    const plannerSession = `roster-${randomBytes(8).toString('hex')}-planner`;
    const planner = await liveLog.seat('planner', plannerSession, config, (onEvent) => runPlanner({
      worktree: worktreePath, repoRoot, ask: document.ask, title: document.title, reference: `local:${task}`,
      task, session: plannerSession, config, env, fetchImpl, vault, onEvent, askKind, retryCommand, signal,
    }));
    if (journalEnabled) await recordRun({ task, session: plannerSession, provider: planner.run?.provider,
      seat: 'planner', delivery: liveLog.delivery('planner') }, {
      cwd: worktreePath, env: { ...metricEnv, ...planner.run?.env }, run: planner.run,
    });
    log(`PLAN: ${planner.planPath}\nReview child issue drafts on GitHub and run each slice separately. No coder or publisher ran.`);
    return { worktreePath, task, session, planner, planPath: planner.planPath, askKind, classification,
      planningOnly: true, command: null, failed: false, logPath: liveLog.path, logSession: liveLog.session };
  }
  const record = async (result) => {
    if (!journalEnabled) return;
    await recordRun({
      task, session, task_class: result.taskMetadata?.task_class,
      seat: 'coder', delivery: liveLog.delivery('coder', {
        ...(result.taskMetadata?.estimate_min != null ? { estimate_min: result.taskMetadata.estimate_min } : {}),
        review_repairs: liveLog.delivery('coder')?.attempt > 1 ? 1 : 0,
      }),
      provider: result.run?.provider,
      ...excellenceFields(result.excellence, { config, env }),
    }, { cwd: worktreePath, env: { ...metricEnv, ...result.run?.env }, run: result.run });
  };
  const reviewSeat = async (result) => {
    const reviewConfig = { ...config, llm: { ...config.llm,
      model: result.mode === 'llm' ? result.model : config.llm.model } };
    const review = await liveLog.seat('reviewer', reviewerSession, reviewConfig, (onEvent) => runReviewer({
      worktree: worktreePath, repoRoot, config: reviewConfig, coderResult: result,
      env, fetchImpl, vault, onEvent, askKind, retryCommand, signal,
    }));
    const reviewRun = review.queried ? buildRun({
      config: reviewConfig, response: review.response, task, session: reviewerSession, env,
    }) : null;
    if (journalEnabled) await recordRun({ task, session: reviewerSession, provider: reviewRun?.provider,
      seat: 'reviewer', delivery: liveLog.delivery('reviewer') }, {
      cwd: worktreePath, env: { ...metricEnv, ...reviewRun?.env }, run: reviewRun,
    });
    return { review, reviewRun };
  };
  const coderSeat = (priorFeedback = null) =>
    liveLog.seat('coder', session, config, (onEvent) => runCoder({
      worktree: worktreePath, repoRoot, config, env, task, session,
      fetchImpl, vault, runTestCommand, priorFeedback, onEvent, askKind, retryCommand, signal, steeringControl,
    }));
  let result;
  try {
    result = await coderSeat();
  } catch (error) {
    if (error instanceof Error && error.result) {
      await record(error.result);
    }
    throw error;
  }
  await record(result);
  let { review, reviewRun } = await reviewSeat(result);
  const boundedTask = taskFilesAllowed(taskSource).length === 1;
  const docsOnly = boundedTask && taskFilesAllowed(taskSource).every((file) => file.endsWith('.md'));
  if (boundedTask && !docsOnly && result.excellence.pass && review.queried && review.verdict === 'fail') {
    await archiveRunArtifacts(worktreePath, {
      task,
      git: (args) => git(worktreePath, args, withoutLlmKeys(env, config)),
      preserve: ['TASK.md'],
    });
    result = await coderSeat(review.content);
    await record(result);
    ({ review, reviewRun } = await reviewSeat(result));
  }
  log(`Worktree: ${worktreePath}\nTASK: ${path.join(worktreePath, 'TASK.md')}\n` +
    `Live log: ${liveLog.path}\n` +
    `CONTEXT: ${result.contextPath}\n` + (result.researchPath ? `RESEARCH: ${result.researchPath}\n` : '') +
    `RESULT: ${result.resultPath}\n` +
    `REVIEW: ${review.reviewPath} (${review.verdict})\n` +
    `Mode: ${result.mode}\n` + (result.run ? `AI-Run: ${result.run.line}\n` : '') +
    (reviewRun ? `Reviewer AI-Run: ${reviewRun.line}\n` : '') +
    'Single coder task complete; publication remains an explicit reviewed App SDK handoff.');
  return { worktreePath, task, session, reviewerSession, result, review, run: result.run,
    reviewerRun: reviewRun, askKind, classification, logPath: liveLog.path, logSession: liveLog.session };
}

async function claimIssue(issueNumber, cwd, command) {
  const common = await command('git', ['rev-parse', '--git-common-dir'], cwd);
  if (typeof common !== 'string' || !common.trim()) throw new Error('Issue claim requires a valid Git common directory');
  return acquireRepoLock(`issue-${issueNumber}`, {
    lockRoot: path.join(path.resolve(cwd, common.trim()), 'roster', 'wave-runs'),
  });
}

async function runClaimedAssignment(issueNumber, options) {
  let claim;
  try {
    return await runTrackedAssignment(issueNumber, { ...options,
      [claimHook]: async (cwd, command) => { claim = await claimIssue(issueNumber, cwd, command); },
    });
  } finally {
    await claim?.release();
  }
}

export async function runBuiltinIssue(issueNumber, options = {}) {
  const attempts = attemptLimit(options.attempts, options.config?.seat?.max_attempts ?? 16);
  if (attempts > 1 && (options.confirm || options.planMode || options.acceptPlan ||
      options.skipReview || options.parallel > 1 || options.steeringControl)) {
    throw new TypeError('Multiple attempts require a reviewed slice without plan, confirm, steering or parallel waves');
  }
  validateIssueNumber(issueNumber);
  const parallel = parallelLimit(options.parallel);
  if (parallel > 1 && (options.confirm || options.planMode || options.steeringControl)) {
    throw new TypeError('Parallel waves cannot combine with confirmation, plan mode, or shared coder steering');
  }
  const result = await runClaimedAssignment(issueNumber, options);
  if (parallel > 1 && result?.waves?.length && !result.failed && !result.planMode) {
    const { env = process.env, log = console.log } = options;
    const config = options.config ?? loadConfig({ repoRoot: options.repoRoot ?? rosterRoot, cwd: result.repoRoot });
    const fleet = options.autoModel ? await loadFleet({ cwd: result.repoRoot }) : null;
    const capacity = fleet ? fleet.profiles.reduce((total, profile) => total + profile.concurrency, 0)
      : config.llm.concurrency ?? 1;
    const command = options.runCommand ?? (async (program, args, cwd) =>
      (await execFileAsync(program, args, { cwd, env: withoutLlmKeys(env, config), encoding: 'utf8', signal: options.signal })).stdout);
    const { preparedRun, onPrepared, planMode, acceptPlan, parallel: _parallel, ...childOptions } = options;
    const scheduled = await runReadyWaves({ parallel, capacity, signal: options.signal,
      readBoard: () => waveBoard({ worktree: result.worktreePath, cwd: result.repoRoot, env,
        apiKeyEnv: config.llm.api_key_env, runCommand: command }),
      onState: options.onRunEvent,
      runChild: async (row) => {
        const prefix = `[issue-${row.issue}] `;
        try {
          return await runClaimedAssignment(row.issue, { ...childOptions,
            cwd: result.repoRoot,
            log: (message) => log(String(message).split('\n').map((line) => prefix + line).join('\n')),
            errorOutput: { write: (text) => (options.errorOutput ?? process.stderr).write(
              String(text).replace(/^/gm, prefix)) },
            onRunEvent: (event) => options.onRunEvent?.({ ...event, issue: row.issue, wave: row.wave }),
          });
        } catch (error) {
          log(prefix + 'Run failed: ' + redactEvidence(String(error?.message ?? error), { env, apiKeyEnv: config.llm.api_key_env }));
          throw error;
        }
      },
    });
    log(`Parallel waves: ${scheduled.children.length} attempted, limit ${scheduled.parallel}; ` +
      `${scheduled.children.filter((child) => child.status === 'rejected').length} rejected.`);
    if (scheduled.boardError) log('Wave board refresh failed: ' +
      redactEvidence(scheduled.boardError.message, { env, apiKeyEnv: config.llm.api_key_env }));
    const completed = { ...result, ...scheduled, parallelRun: true, planningOnly: scheduled.children.length === 0,
      continueWith: undefined, command: null };
    const updateStatus = options.issueStatus === undefined ? setIssueRunStatus : options.issueStatus;
    if (env.GITHUB_APP_ID && env.GITHUB_APP_PRIVATE_KEY_PATH && result.issue?.url && typeof updateStatus === 'function') {
      const [status, detail] = runOutcomeStatus(completed, { published: options.publish });
      try {
        await updateStatus({ issue: result.issue, status, detail,
          repoRoot: result.repoRoot, cwd: options.cwd ?? process.cwd(), env });
      } catch (error) {
        log('Parent issue status not updated: ' + redactEvidence(String(error?.message ?? error),
          { env, apiKeyEnv: config.llm.api_key_env }));
      }
    }
    return completed;
  }
  if (!result?.continueWith) return result;
  // A split feature is delivered, not parked: its next open wave slice runs in its own issue worktree.
  const { preparedRun, onPrepared, planMode, acceptPlan, ...childOptions } = options;
  const child = await runClaimedAssignment(result.continueWith, childOptions);
  return { ...child, parent: { issue: issueNumber, planPath: result.planPath, waves: result.waves } };
}

export function runOutcomeStatus(result, { published }) {
  const pause = (reason) => ['blocked', [`Waiting on a human: ${reason}`]];
  if (result.clarification) return ['blocked', ['The Ask needs clarification before planning.',
    String(result.clarification).split('\n')[0]]];
  if (result.failed) return ['blocked', [result.planner?.error ? `Planning failed: ${result.planner.error}`
    : 'The run reported a failure; see the run log.']];
  if (result.confirmedPause) return pause('the TASK was paused by --confirm.');
  if (result.planMode) return pause('plan mode wrote PLAN.md only.');
  if (result.parallelRun) return [result.failed ? 'blocked' : 'review',
    [`Parallel child runs: ${result.children.length}. Child issues retain their own review/publication status.`]];
  if (result.planningOnly && result.waves?.length && result.waves.every((row) => row.state === 'done')) {
    return ['review', [`Every child issue is closed: ${result.waves.map((row) => `#${row.issue}`).join(', ')}.`,
      'Close this parent after human AI-Eval of the delivered slices.']];
  }
  if (result.planningOnly) return pause('review the PLAN and child issue drafts, then run each slice.');
  const verdict = result.review?.verdict;
  if (published) return ['review', ['Published and merged; awaiting human AI-Eval.',
    `Review: ${verdict ?? 'not run'}.`]];
  if (verdict === 'pass') {
    return ['review', [`Review: pass.`, result.run?.metrics?.model ? `Coder model: ${result.run.metrics.model}` : null,
      'Reviewed worktree is ready to publish.'].filter(Boolean)];
  }
  return ['blocked', [`Review: ${verdict ?? 'not run'}.`, ...(result.review?.reasons ?? []).slice(0, 4)]];
}

// When the machine holds App credentials, every issue run reports itself on the issue (spec section 3:
// the board is GitHub). Labels let other agents and humans see claimed, review, and blocked work.
// Board updates are best-effort: a GitHub outage never fails or blocks the run itself.
async function runTrackedAssignment(issueNumber, options) {
  const { issueStatus = setIssueRunStatus, env = process.env, log = console.log, cwd = process.cwd(),
    onPrepared, publish = false } = options;
  if (typeof issueStatus !== 'function' || !env.GITHUB_APP_ID || !env.GITHUB_APP_PRIVATE_KEY_PATH) {
    return runBuiltinAssignment(issueNumber, options);
  }
  let prepared = null;
  const redact = (text) => redactEvidence(text, { env, apiKeyEnv: options.config?.llm?.api_key_env });
  const post = async (status, detail, extra = {}) => {
    if (!prepared || prepared.local || !prepared.issue?.url) return null;
    try {
      return await issueStatus({ issue: prepared.issue, status, detail, repoRoot: prepared.repoRoot,
        cwd, env, ...extra });
    } catch (error) {
      log(`Issue #${prepared.issue.number} status not updated (${status}): ${redact(String(error?.message ?? error))}`);
      return null;
    }
  };
  let result;
  try {
    result = await runBuiltinAssignment(issueNumber, { ...options, async onPrepared(ready) {
      prepared = ready;
      await onPrepared?.(ready);
      const posted = await post('in-progress', [`Branch: ${ready.task}`,
        ready.start ? formatStart(ready.start, ready.drift) : null,
        `Seats: ${options.seats ?? 'planner,coder,reviewer'}`,
        options.autoModel ? 'Model routing: auto' : null].filter(Boolean));
      if (posted?.claimed) {
        log(ready.reused
          ? `Resuming #${ready.issue.number}: its roster:in-progress claim and worktree are from an earlier run on this machine.`
          : `Issue #${ready.issue.number} was already labeled roster:in-progress; another agent may be working it.`);
      }
    } });
  } catch (error) {
    await post('blocked', [options.signal?.aborted ? 'The run was cancelled by the operator.'
      : `Run stopped: ${redact(String(error?.message ?? error))}`]);
    throw error;
  }
  // A feature parent stays claimed while its next wave slice runs and reports on its own issue.
  if (result?.continueWith) return result;
  // A completed publish run has merged its PR; the merge comment already reports it, so only the label moves.
  const published = publish && Boolean(result) && !result.planningOnly && !result.failed;
  const [status, detail] = runOutcomeStatus(result ?? {}, { published });
  await post(status, detail.map((line) => redact(String(line))),
    published ? { comment: false } : {});
  return result;
}

export async function runBuiltinAsk(ask, options = {}) {
  return runBuiltinAssignment(null, { ...options, ask: cleanAskText(ask), publish: false });
}

async function prepareLocalAsk(ask, { cwd, config, runCommand, start: startOverrides }) {
  const repoRoot = path.resolve((await runCommand('git', ['rev-parse', '--show-toplevel'], cwd)).trim());
  const task = `local-${randomBytes(8).toString('hex')}`;
  const worktrees = config.paths.worktrees;
  if (typeof worktrees !== 'string' || !worktrees || path.isAbsolute(worktrees) ||
      path.win32.isAbsolute(worktrees) ||
      worktrees.split(/[\\/]/).some((part) => !part || part === '.' || part === '..')) {
    throw new TypeError('Worktrees path must be relative to the repository root');
  }
  const worktreePath = path.join(repoRoot, worktrees, task);
  await ensureLocalPath(worktreePath, repoRoot);
  const start = await resolveStart({ repoRoot, runCommand, ...startOptions(config, startOverrides) });
  await runCommand('git', ['worktree', 'add', '-b', task, worktreePath, ...(start.ref ? [start.ref] : [])], repoRoot);
  await initializeWorktreeSubmodules(worktreePath, runCommand);
  const assignmentPath = path.join(worktreePath, 'ASSIGNMENT.md');
  await fs.writeFile(assignmentPath, `# Local Ask\n\n${ask}\n`, { flag: 'wx' });
  return { repoRoot, worktreePath, task, session: `roster-${task}-coder`, ask, assignmentPath,
    issue: { title: ask.split('\n')[0], body: ask }, local: true, reused: false, start, drift: null };
}

async function reusePreparedAssignment(run, { cwd, config, runCommand, ask, issueNumber }) {
  const root = path.resolve((await runCommand('git', ['rev-parse', '--show-toplevel'], cwd)).trim());
  const expectedTask = issueNumber === null ? run?.task : `issue-${issueNumber}`;
  if (!run || typeof run.ask !== 'string' || typeof run.worktreePath !== 'string' ||
      typeof run.repoRoot !== 'string' || !run.issue || run.task !== expectedTask ||
      (issueNumber === null
        ? !run.local || !/^local-[a-f0-9]{16}$/.test(run.task) || run.ask !== ask
        : run.local || run.issue.number !== Number(issueNumber))) {
    throw new Error('Retry requires the unchanged prepared Ask and task worktree');
  }
  const selected = run.attempt ? selectedAttemptRecord(loadLearning({ cwd: root }).runs, run.task) : null;
  if (run.attempt && (!selected || selected.session !== run.session ||
      selected.attempt.index !== run.attempt.index)) {
    throw new Error('Retry must use the recorded selected attempt, not a losing worktree');
  }
  const branch = selected ? `${run.task}-a${selected.attempt.index}` : run.task;
  const expectedPath = path.resolve(root, config.paths.worktrees, branch);
  const same = (left, right) => process.platform === 'win32'
    ? path.resolve(left).toLowerCase() === path.resolve(right).toLowerCase()
    : path.resolve(left) === path.resolve(right);
  if (!same(root, run.repoRoot) || !same(expectedPath, run.worktreePath)) {
    throw new Error('Retry worktree does not belong to the current repository and task');
  }
  await ensureLocalPath(expectedPath, root);
  const inventory = await runCommand('git', ['worktree', 'list', '--porcelain', '-z'], root);
  const registered = inventory.split('\0\0').some((record) => {
    const fields = record.split('\0');
    const worktree = fields.find((field) => field.startsWith('worktree '))?.slice(9);
    return worktree && same(worktree, expectedPath) && fields.includes(`branch refs/heads/${branch}`);
  });
  if (!registered) throw new Error('Retry worktree registration or branch changed; refusing a second worktree');
  const file = path.join(expectedPath, 'ASSIGNMENT.md');
  await ensureLocalPath(file, root);
  const entry = await fs.lstat(file);
  if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1 || entry.size > 65536) {
    throw new Error('Retry assignment must be a regular single-link file');
  }
  const expected = run.local ? `# Local Ask\n\n${run.ask}\n` : renderAssignment(run.issue);
  if ((await fs.readFile(file, 'utf8')).replaceAll('\r\n', '\n') !== expected.replaceAll('\r\n', '\n')) {
    throw new Error('Retry assignment changed; refusing to replace the prepared Ask');
  }
  await initializeWorktreeSubmodules(expectedPath, runCommand);
  return { repoRoot: root, worktreePath: expectedPath, task: run.task, ask: run.ask, issue: run.issue,
    session: run.session, assignmentPath: file, envPath: run.envPath, metadata: run.metadata,
    local: run.local, reused: true, ...(run.attempt ? { attempt: run.attempt } : {}) };
}

function taskSummary(planner, effort) {
  const document = parseTaskDocument(planner.task);
  const docsOnly = document.files_allowed.length > 0 && document.files_allowed.every((file) => file.endsWith('.md'));
  const checks = document.acceptance_checks.filter((check) => !(docsOnly && /node --test/.test(check)));
  const shownEffort = effort === 'none' && docsOnly ? 'l' : effort;
  return `Task summary:\nOutcome: ${document.title}\n` +
    `Allowed files: ${document.files_allowed.join(', ')}\n` +
    `Checks:\n${checks.map((check) => `- ${check}`).join('\n')}\n` +
    `Effort: ${shownEffort}\n`;
}

async function executableWavePlan(worktree, reference) {
  try {
    const { plan } = await readWavePlan(worktree);
    if (plan.reference !== reference || !plan.issues.every((issue) => issue.files_allowed.length)) return null;
    return { reused: true, askKind: plan.kind, planPath: path.join(worktree, 'PLAN.md'), outline: plan,
      planningOnly: true, run: null, turns: 0, usage: null };
  } catch {
    return null;
  }
}

async function openWaves({ worktree, cwd, env, apiKeyEnv, runCommand, log }) {
  const rows = await waveBoard({ worktree, cwd, env, apiKeyEnv, open: true, runCommand });
  log('Child issues:\n' + rows.map((row) => `- wave:${row.wave} #${row.issue} ${row.state}: ${row.title}`).join('\n'));
  const next = rows.find((row) => row.state === 'todo');
  const reason = rows.every((row) => row.state === 'done') ? 'every child issue is closed.'
    : 'open children are in review or blocked by an earlier open wave; publish, merge, and close them first.';
  return { rows, next, reason };
}

async function runBuiltinAssignment(issueNumber, options = {}) {
  const { env = process.env, log = console.log, onPrepared } = options;
  const lifecycle = { runId: `run-${randomBytes(8).toString('hex')}`, store: null, prepared: null,
    seats: new Map() };
  const timestamp = () => (options.now?.() ?? new Date()).toISOString();
  const capture = async (event, payload = {}) => {
    const prepared = lifecycle.prepared;
    if (!prepared) return;
    const recorded = await captureLifecycleEvent(lifecycle.store, {
      event, runId: lifecycle.runId, sessionId: prepared.session,
      payload: { issue: prepared.issue.number ?? null, task: prepared.task, ...payload },
    });
    if (!recorded.durable && !provenanceOptOut(env)) {
      log(`Provenance event ${event} was not persisted; durable history is incomplete.`);
    }
  };
  let result;
  try {
    result = await executeBuiltinAssignment(issueNumber, { ...options, lifecycle,
      async onRunEvent(event) {
        if (event.type === 'seat-start') {
          lifecycle.seats.set(event.seat, { requestedModel: event.model,
            startedAt: timestamp(), endedAt: null });
        } else if (['seat-end', 'seat-error'].includes(event.type)) {
          const timing = lifecycle.seats.get(event.seat);
          if (timing) timing.endedAt = timestamp();
        }
        await options.onRunEvent?.(event);
      },
      async onPrepared(prepared) {
        lifecycle.prepared = prepared;
        lifecycle.store = provenanceOptOut(env) ? null
          : provenanceStoreForRun({ repoRoot: prepared.repoRoot, env });
        await capture('started');
        await onPrepared?.(prepared);
      },
    });
  } catch (error) {
    await capture(isRunCancelled(error) || options.signal?.aborted ? 'cancellation' : 'failure');
    throw error;
  }
  await capture(result.failed ? 'failure' : result.planningOnly || result.confirmedPause
    ? 'session' : 'completed', { outcome: result.failed ? 'fail'
      : result.planningOnly || result.confirmedPause ? 'paused'
      : result.result?.mode === 'stub' ? 'unverified' : 'completed' });
  return result;
}

async function executeBuiltinAssignment(issueNumber, {
  cwd = process.cwd(),
  repoRoot = rosterRoot,
  config = loadConfig({ repoRoot, cwd }),
  env = process.env,
  publish = false,
  seats = 'planner,coder,reviewer',
  autoModel = false,
  skipReview = false,
  confirm = false,
  ask,
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
  debug = createDebugLog({ env }),
  onRunEvent,
  signal,
  preparedRun,
  onPrepared,
  planMode = false,
  acceptPlan = false,
  steeringControl,
  attempts = 1,
  start,
  lifecycle,
  [claimHook]: beforePreparation,
} = {}) {
  throwIfCancelled(signal);
  config = { ...config, capabilities: config.capabilities ?? await loadCapabilities({ cwd }) };
  attempts = attemptLimit(attempts, config.seat.max_attempts ?? 3);
  if (attempts > 1 && (issueNumber === null || confirm || planMode || acceptPlan || skipReview ||
      steeringControl || !['planner,coder', 'planner,coder,reviewer'].includes(seats))) {
    throw new TypeError('Multiple attempts require reviewed GitHub slices without interactive or planning modes');
  }
  const retryCommand = issueNumber === null ? retryCommandForTask(null) :
    `${retryCommandForTask(`issue-${issueNumber}`)}${autoModel ? ' --auto-model' : ''}`;
  if (!autoModel && !config.llm.model && (env.AI_MODEL || env.ROSTER_MODEL)) {
    config = { ...config, llm: Object.freeze({ ...config.llm, model: resolvePublishModel({ config, env }) }) };
  }
  if (!['planner,coder', 'planner,coder,reviewer'].includes(seats)) {
    throw new TypeError('Builtin seats must be planner,coder,reviewer in that order');
  }
  if (typeof autoModel !== 'boolean') throw new TypeError('--auto-model must be a boolean');
  if (typeof skipReview !== 'boolean') throw new TypeError('--skip-review must be a boolean');
  if (typeof confirm !== 'boolean') throw new TypeError('--confirm must be a boolean');
  if (typeof planMode !== 'boolean' || typeof acceptPlan !== 'boolean') throw new TypeError('Plan mode must be a boolean');
  if (planMode && (publish || confirm || acceptPlan)) throw new TypeError('--plan cannot combine with --publish, --confirm, or plan acceptance');
  if (confirm && publish) throw new TypeError('--confirm cannot be combined with --publish');
  if (publish) requirePublicationEnabled(config);
  const reviewBypass = skipReview || !isReviewRequired(config);
  const fleet = autoModel || attempts > 1 ? await loadFleet({ cwd }) : null;
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
  const issueCommand = async (program, args, workingDirectory) => {
    throwIfCancelled(signal);
    try {
      const result = runCommand ? await runCommand(program, args, workingDirectory)
        : (await execFileAsync(program, args, { cwd: workingDirectory, env: commandEnv, encoding: 'utf8', signal })).stdout;
      throwIfCancelled(signal);
      return result;
    } catch (error) {
      throwIfCancelled(signal);
      throw error;
    }
  };
  await beforePreparation?.(cwd, issueCommand);
  const startChoice = startOptions(config, start ?? {});
  const prepared = preparedRun ? await reusePreparedAssignment(preparedRun, {
    cwd, config, runCommand: issueCommand, ask, issueNumber,
  }) : issueNumber === null ? await prepareLocalAsk(ask, { cwd, config, runCommand: issueCommand, start: startChoice })
    : await runIssue(issueNumber, {
    cwd, runCommand: issueCommand, worktrees: config.paths.worktrees, log: () => {}, now, config,
    beforeWorktree: async (root, worktreePath, issue, repository) => {
      await ensureLocalPath(worktreePath, root);
      await requireEarlierWavesClosed({ issue, repository, cwd: root, runCommand: issueCommand });
    },
    sessionId: `roster-${issueNumber}-coder`, recordPreparation: false, start: startChoice,
  });
  if (prepared.start) log(formatStart(prepared.start, prepared.drift));
  if (preparedRun && !prepared.local) {
    const repository = githubRepository((await issueCommand('git', ['remote', 'get-url', 'origin'], prepared.repoRoot)).trim());
    await requireEarlierWavesClosed({ issue: prepared.issue, repository, cwd: prepared.repoRoot, runCommand: issueCommand });
  }
  await onPrepared?.(prepared);
  const reference = prepared.local ? `local:${prepared.task}` : `issue:${prepared.issue.number}`;
  const sessionPrefix = prepared.local ? `roster-${prepared.task}` : `roster-${prepared.issue.number}`;
  const { worktreePath } = prepared;
  await requireWorktreeCheckout(worktreePath, env);
  let classification = classifyAsk(prepared.ask, { title: prepared.issue.title });
  let activeConfig = config;
  let autoRecommendation = null;
  let route = null;
  const routeAttempts = [];
  const routingTaskClass = prepared.metadata?.task_class ?? inferTaskClass(prepared.issue.title) ?? 'feat';
  const chooseFleetRoute = (excludedProfileIds = []) => routeTask({
    cwd: prepared.repoRoot, installationRoot: repoRoot, fleet,
    taskClass: routingTaskClass, difficulty: prepared.metadata?.difficulty ?? 2,
    records: metricsLoader({ contractsPath, cwd: prepared.repoRoot }),
    excludedProfileIds: [...new Set(excludedProfileIds.filter(Boolean))],
  });
  const routeLlm = (selectedRoute) => ({
    ...withFleetProfile(config, selectedRoute.profile).llm,
    effort: selectedRoute.recommendation?.effort ?? config.llm.effort,
    locked_model: selectedRoute.profile.model,
  });
  const selectAutoRoute = async (excludedProfileIds = []) => {
    const selectedRoute = await chooseFleetRoute(excludedProfileIds);
    if (!selectedRoute) return null;
    const selected = withFleetProfile(config, selectedRoute.profile);
    activeConfig = { ...selected, llm: Object.freeze(routeLlm(selectedRoute)) };
    autoRecommendation = selectedRoute.recommendation;
    route = selectedRoute;
    return selectedRoute;
  };
  // A route failure (gateway error, timeout, stall) is evidence about the endpoint, not the task:
  // exclude that profile for this run and continue the same seat on a different eligible profile.
  const recoverRoute = async (seat, error) => {
    const failure = autoModel && route ? routeFailure(error) : null;
    if (!failure) return false;
    const failedProfile = route.profile.id;
    routeAttempts.push({ seat, profile: failedProfile, ...failure });
    const detail = failure.reason === 'endpoint-error'
      ? 'gateway returned an error twice' : 'endpoint timed out or stalled';
    const retryEffort = activeConfig.llm.review_retry_effort;
    const alternate = await selectAutoRoute([...new Set(routeAttempts.map((attempt) => attempt.profile))]);
    if (!alternate) {
      log(`Route recovery exhausted: seat=${seat} profile=${failedProfile} ${detail}; ` +
        'no other eligible fleet profile remains for this run.');
      return false;
    }
    activeConfig = selectReasoning({ ...activeConfig, llm: { ...activeConfig.llm,
      ...(retryEffort ? { review_retry_effort: retryEffort } : {}),
    } }, { kind: classification.kind, taskClass: prepared.metadata?.task_class,
      difficulty: prepared.metadata?.difficulty });
    log(`Route recovery: seat=${seat} profile=${failedProfile} ${detail}; ` +
      `continuing with profile=${alternate.profile.id} model=${alternate.profile.model}.`);
    onRunEvent?.({ type: 'route-recovery', seat, failedProfile, ...failure,
      profile: alternate.profile.id, model: alternate.profile.model });
    onRunEvent?.({ type: 'route', model: alternate.profile.model,
      host: new URL(alternate.profile.base_url).host, contextMax: alternate.profile.context_max,
      hardware: alternate.profile.hardware });
    return true;
  };
  if (autoModel) {
    route = await selectAutoRoute();
    if (route) {
      log(`Route: ${formatRoute(route, routingTaskClass).trimEnd()}`);
      onRunEvent?.({ type: 'route', model: route.profile.model,
        host: new URL(route.profile.base_url).host, contextMax: route.profile.context_max,
        hardware: route.profile.hardware });
    } else if (config.llm.base_url && config.llm.model) {
      log(`Route: no eligible fleet profile; keeping saved ${config.llm.model}`);
    } else {
      activeConfig = { ...config, llm: Object.freeze({ ...config.llm, base_url: '', model: '' }) };
      log('Route: no eligible fleet profile or evidence; deterministic stub');
    }
  }
  const existing = !planMode && !acceptPlan && prepared.reused && ['slice', 'clarify'].includes(classification.kind) ? await readPlannerHandoff({
    worktree: worktreePath, reference, ask: prepared.ask, lockedModel: route?.profile.model,
    issueTitle: prepared.issue.title, issueBody: prepared.issue.body,
  }) : { plan: null };
  if (classification.kind === 'clarify' && existing.plan) {
    classification = classifyAsk(prepared.ask, { title: prepared.issue.title,
      filesAllowed: taskFilesAllowed(existing.plan.task) });
  }
  const askKind = classification.kind;
  const previousReview = prepared.reused ? await readPreviousReview(worktreePath) : null;
  const previousCoder = previousReview?.startsWith('# Review\n\nVerdict: fail\n')
    ? loadLearning({ cwd: prepared.repoRoot }).runs.findLast((run) => run.session === `${sessionPrefix}-coder`)
    : null;
  activeConfig = selectReasoning({ ...activeConfig, llm: { ...activeConfig.llm,
    ...(previousCoder && ['l', 'm', 'h', 'x'].includes(previousCoder.effort)
      ? { review_retry_effort: previousCoder.effort } : {}),
  } }, { kind: askKind, taskClass: prepared.metadata?.task_class,
    difficulty: prepared.metadata?.difficulty });
  const sessions = {
    planner: `${sessionPrefix}-planner`,
    coder: prepared.session,
    reviewer: `${sessionPrefix}-reviewer`,
  };
  log(`Ask kind: ${askKind} (${classification.reason})`);
  if (askKind === 'clarify') {
    if (attempts > 1) throw new TypeError('Multiple attempts require a bounded slice');
    log(clarificationHint);
    return { ...prepared, askKind, classification, clarification: clarificationHint, sessions,
      runs: { planner: null, coder: null, reviewer: null }, run: null, command: null,
      planningOnly: true, failed: false };
  }
  if (existing.reason) log(existing.reason);
  // Reuse an executable feature PLAN on rerun so its GitHub-linked child issues keep matching.
  const existingWavePlan = !planMode && !acceptPlan && !prepared.local && prepared.reused &&
    askKind === 'feature' ? await executableWavePlan(worktreePath, reference) : null;
  const plannerPreserve = acceptPlan || existingWavePlan ? ['PLAN.md'] : existing.plan
    ? existing.plan.normalizedRecipe ? ['TASK.md'] : ['RECIPE.yml', 'TASK.md'] : [];
  const archivePath = prepared.reused ? await archiveRunArtifacts(worktreePath, {
    task: prepared.task, git: (args) => git(worktreePath, args, commandEnv),
    preserve: plannerPreserve,
  }) : null;
  if (archivePath) log(`Previous generated run artifacts preserved: ${archivePath}`);
  const liveLog = await createRunLog({
    repoRoot: prepared.repoRoot, session: prepared.session, env,
    apiKeyEnv: activeConfig.llm.api_key_env, errorOutput, now,
    debug, issue: prepared.issue.number ?? null,
    observe: onRunEvent,
  });
  const metricEnv = { ...commandEnv };
  for (const name of [...RUN_ENV_NAMES, 'GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH',
    'GH_TOKEN', 'GITHUB_TOKEN']) delete metricEnv[name];
  let planner;
  for (;;) {
    try {
      planner = existingWavePlan ? existingWavePlan : acceptPlan ? await acceptPlannerPlan({ worktree: worktreePath, ask: prepared.ask,
        title: prepared.issue.title, reference, learningRoot: prepared.repoRoot, config: activeConfig, env, signal,
        lockedModel: route?.profile.model })
        : existing.plan ? await preparePlannerHandoff(existing.plan, {
        worktree: worktreePath, learningRoot: prepared.repoRoot, config: activeConfig, env,
      }) : await liveLog.seat('planner', sessions.planner, activeConfig, (onEvent) => runPlanner({
        worktree: worktreePath, repoRoot, issue: prepared.issue, config: activeConfig,
        ask: prepared.ask, reference, metadata: prepared.metadata ?? undefined, task: prepared.task,
        session: sessions.planner, fetchImpl, env, vault, learningRoot: prepared.repoRoot,
        lockedModel: route?.profile.model, onEvent, askKind, retryCommand, signal, planMode,
      }));
      if (planner.reused && askKind === 'slice') log('planner skipped artifacts valid; starting coder.');
      break;
    } catch (error) {
      if (error instanceof Error && error.run) {
        await recordRun({ session: sessions.planner, task: prepared.task,
          seat: 'planner', delivery: liveLog.delivery('planner'),
          task_class: prepared.metadata?.task_class ?? inferTaskClass(prepared.issue.title) }, {
          cwd: prepared.repoRoot, env: { ...metricEnv, ...error.run.env }, createDirectory: true, run: error.run,
        });
      }
      if (!await recoverRoute('planner', error)) throw error;
      // A partial handoff from the failed route must not block the alternate planner's exclusive writes.
      await archiveRunArtifacts(worktreePath, {
        task: prepared.task, git: (args) => git(worktreePath, args, commandEnv), preserve: plannerPreserve,
      });
    }
  }
  if (askKind === 'slice' && !planMode && !planner.error) {
    const { critiquePlannerHandoff } = await import('../seats/planner.mjs');
    planner = await critiquePlannerHandoff(planner, {
      worktree: worktreePath, ask: prepared.ask, title: prepared.issue.title, reference,
      learningRoot: prepared.repoRoot, config: activeConfig, env, fetchImpl, vault, signal,
      session: sessions.planner, task: prepared.task,
      retryCommand, lockedModel: route?.profile.model, onEvent: onRunEvent,
    });
  }
  const taskClass = askKind === 'slice' && !planMode ? planner.metadata.task_class
    : prepared.metadata?.task_class ?? inferTaskClass(prepared.issue.title) ?? 'feat';
  const provenanceRunId = lifecycle.runId;
  const provenance = lifecycle.store;
  const reviewRepairs = [];
  let recordedReviewRepairs = 0;
  const delivery = {};
  const recordSeat = async (session, run, excellence, seatConfig = activeConfig) => {
    const seat = Object.keys(sessions).find((name) => sessions[name] === session);
    const evidence = liveLog.delivery(seat, {
      ...(planner.metadata?.estimate_min != null ? { estimate_min: planner.metadata.estimate_min } : {}),
      ...(seat === 'coder' ? { review_repairs: reviewRepairs.length - recordedReviewRepairs } : {}),
    });
    await recordRun({
      session, task: prepared.task, task_class: taskClass, provider: run?.provider,
      seat, delivery: evidence,
      ...(excellence ? excellenceFields(excellence, { config, env }) : {}),
    }, { cwd: prepared.repoRoot, env: { ...metricEnv, ...run?.env }, createDirectory: true, run });
    if (seat === 'coder') recordedReviewRepairs = reviewRepairs.length;
    delivery[seat] = evidence;
    // Durable machine history for `roster history`; best-effort so provenance can never fail a run.
    const recorded = await captureLifecycleEvent(provenance, { event: 'session', runId: provenanceRunId, sessionId: session, payload: {
      issue: prepared.issue.number ?? null, task: prepared.task, task_class: taskClass,
      seat: Object.keys(sessions).find((name) => sessions[name] === session) ??
        (session.endsWith('-coder') ? 'coder' : session.endsWith('-reviewer') ? 'reviewer' : null),
      model: run?.env?.AI_MODEL ?? null, provider: run?.provider ?? null,
      ...lifecycle.seats.get(seat),
      servedModel: run?.metrics?.model ?? '',
      route: { name: seatConfig.llm.fleet_profile ?? seatConfig.llm.profile ?? '',
        ...(seatConfig.llm.fleet_profile ? { profile: seatConfig.llm.fleet_profile } : {}),
        ...(seatConfig.llm.hardware ? { hardware: seatConfig.llm.hardware } : {}) },
      metrics: {
        ...(run?.metrics?.prompt_tokens !== undefined ? { tokens_prompt: run.metrics.prompt_tokens } : {}),
        ...(run?.metrics?.completion_tokens !== undefined
          ? { tokens_completion: run.metrics.completion_tokens } : {}),
        ...(evidence?.duration_ms !== undefined ? { duration_ms: evidence.duration_ms } : {}),
      },
      ...(excellence ? { outcome: excellence.pass ? 'pass' : 'fail' } : {}),
    } });
    if (!recorded.durable && !provenanceOptOut(env)) {
      log(`Provenance session ${session} was not persisted; durable history is incomplete.`);
    }
  };
  const plannerRun = planner.run;
  if (!planner.reused) await recordSeat(sessions.planner, plannerRun);
  for (const run of planner.critic?.runs ?? []) await recordSeat(run.metrics.session, run);
  if (planner.critic?.revisionRun) await recordSeat(`${sessions.planner}-revision`, planner.critic.revisionRun);
  if (planner.error) {
    log(`Planning failed: ${planner.error}\nRECIPE/TASK stubs are unverified; coder, reviewer, tests, and publication did not run.`);
    return {
      ...prepared, askKind, classification, recipePath: planner.recipePath, taskPath: planner.taskPath,
      planner, sessions, runs: { planner: plannerRun, coder: null, reviewer: null },
      run: null, command: null, route, autoRecommendation, archivePath, failed: true, planningOnly: true,
      logPath: liveLog.path, logSession: liveLog.session,
    };
  }
  if (planMode && askKind === 'slice') {
    log(`PLAN: ${planner.planPath}\nPlan mode: no TASK, recipe, product edits, coder, tests, reviewer, or publisher ran.`);
    return { ...prepared, askKind, classification, planner, planPath: planner.planPath, planMode: true,
      planningOnly: true, failed: false, sessions, runs: { planner: plannerRun, coder: null, reviewer: null },
      run: null, command: null, logPath: liveLog.path, logSession: liveLog.session };
  }
  if (askKind !== 'slice') {
    if (attempts > 1) throw new TypeError('Multiple attempts require a bounded slice, not child waves');
    if (existingWavePlan) log(`Reusing executable ${askKind} PLAN with GitHub-linked child issues.`);
    // Features deliver through their slices; initiative drafts are features that are re-planned on their own runs.
    const delivery = askKind === 'feature' && !prepared.local && !confirm &&
      (await executableWavePlan(worktreePath, reference)) ? await openWaves({
        worktree: worktreePath, cwd: prepared.repoRoot, env, apiKeyEnv: activeConfig.llm.api_key_env,
        runCommand: issueCommand, log,
      }).catch((error) => ({ error })) : null;
    if (delivery?.next) {
      log(`PLAN: ${planner.planPath}\nContinuing ${askKind} #${prepared.issue.number} with wave ${delivery.next.wave} ` +
        `child #${delivery.next.issue}: ${delivery.next.title}`);
      return { ...prepared, askKind, classification, planner, planPath: planner.planPath, sessions,
        runs: { planner: plannerRun, coder: null, reviewer: null }, run: null, command: null,
        planningOnly: true, failed: false, archivePath, logPath: liveLog.path, logSession: liveLog.session,
        waves: delivery.rows, continueWith: delivery.next.issue };
    }
    if (delivery?.error) log(`PLAN: ${planner.planPath}\nChild issues could not be opened: ${redactEvidence(delivery.error.message, { env, apiKeyEnv: activeConfig.llm.api_key_env })}\n` +
      `Retry: roster run --issue ${prepared.issue.number} reuses this PLAN; no coder, reviewer, tests, or publisher ran.`);
    else log(delivery ? `PLAN: ${planner.planPath}\nNo child issue is ready to run: ${delivery.reason}`
      : `PLAN: ${planner.planPath}\nReview the ${askKind} child issue drafts and wave labels on GitHub, ` +
      'then run each bounded slice separately. No coder, reviewer, tests, or publisher ran.');
    return { ...prepared, askKind, classification, planner, planPath: planner.planPath, sessions,
      runs: { planner: plannerRun, coder: null, reviewer: null }, run: null, command: null,
      planningOnly: true, failed: Boolean(delivery?.error), archivePath, logPath: liveLog.path, logSession: liveLog.session,
      ...(delivery?.rows ? { waves: delivery.rows } : {}) };
  }
  const executePlanned = async ({
    prepared: executionPrepared = prepared, planner: executionPlanner = planner,
    activeConfig: executionConfig = activeConfig, route: executionRoute = route,
    sessions: executionSessions = sessions, liveLog: executionLog = liveLog, isolated = false,
  } = {}) => {
  const prepared = executionPrepared;
  const planner = executionPlanner;
  let activeConfig = executionConfig;
  let route = executionRoute;
  const sessions = executionSessions;
  const liveLog = executionLog;
  const { worktreePath } = prepared;
  const routeAttempts = isolated ? [] : routeAttemptsForRun;
  const recoverRoute = isolated ? async () => false : async (...args) => {
    const recovered = await recoverRunRoute(...args);
    ({ activeConfig, route } = currentRunRoute());
    return recovered;
  };
  const selectAutoRoute = isolated ? async () => null : async (...args) => {
    const selected = await selectRunRoute(...args);
    ({ activeConfig, route } = currentRunRoute());
    return selected;
  };
  const recipeCoder = parseRecipe(planner.recipe).seats.find(({ id }) => id === 'coder');
  let scopeBudget;
  const buildCoderConfig = (model) => selectReasoning({ ...activeConfig,
    seat: { ...activeConfig.seat, ...(recipeCoder.tools === undefined ? {} : { recipe_tools: recipeCoder.tools }),
      ...(scopeBudget === undefined ? {} : { scope_expansion: scopeBudget }) },
    llm: {
    ...activeConfig.llm, model,
    effort: planner.feedback?.effort ?? activeConfig.llm.effort,
  } }, { kind: askKind, taskClass: planner.metadata.task_class, difficulty: planner.metadata.difficulty });
  let coderConfig = buildCoderConfig(isolated ? activeConfig.llm.model : planner.metadata.model || activeConfig.llm.model);
  if (!planner.error) {
    log(taskSummary(planner, coderConfig.llm.effort));
    if (confirm) {
      log(`Paused by --confirm. TASK: ${planner.taskPath}\nNo coder, reviewer, tests, or publisher ran.`);
      return { ...prepared, askKind, classification, planner, recipePath: planner.recipePath, taskPath: planner.taskPath,
        sessions, runs: { planner: plannerRun, coder: null, reviewer: null }, run: null, command: null,
        planningOnly: true, confirmedPause: true, failed: false, archivePath,
        logPath: liveLog.path, logSession: liveLog.session };
    }
  }
  // Spec 4.8 "Reviewer is not the coder": with auto routing, the reviewer takes the best eligible fleet
  // profile other than the coder's, so a model never grades its own work. Only when no other profile is
  // eligible does it fall back to the coder's model, and the log says so.
  const independentReviewerRoute = (excluded = []) => ((autoModel || isolated) && route ? chooseFleetRoute([
    route.profile.id, ...routeAttempts.map((attempt) => attempt.profile), ...excluded,
  ]) : null);
  const removeIncompleteReview = async (reviewPath) => {
    const expected = path.join(worktreePath, 'REVIEW.md');
    if (path.resolve(reviewPath ?? '') !== path.resolve(expected)) {
      throw new Error('Incomplete REVIEW.md is not the worktree review file');
    }
    await ensureLocalPath(expected, worktreePath);
    const entry = await fs.lstat(expected);
    if (!entry.isFile() || entry.isSymbolicLink()) throw new Error('Incomplete REVIEW.md is not a regular file');
    await fs.unlink(expected);
  };
  const reviewSeat = async (coderResult, previousFindings = []) => {
    let reviewerRoute = null;
    const coderProfile = route?.profile.id;
    // Profiles whose review came back incomplete (unparseable report), kept apart from endpoint route failures.
    const incompleteReviewers = [];
    let independent = await independentReviewerRoute();
    if (autoModel && route) {
      log(independent
        ? `Reviewer route: profile=${independent.profile.id} model=${independent.profile.model} (independent of coder profile=${coderProfile}).`
        : `Reviewer route: no eligible fleet profile other than coder profile=${coderProfile}; reviewing with the coder's model.`);
    }
    for (;;) {
      const reviewConfig = independent ? { ...coderConfig, llm: { ...coderConfig.llm, ...routeLlm(independent) } }
        : reviewerRoute ? { ...coderConfig, llm: { ...coderConfig.llm, ...activeConfig.llm } }
        : { ...coderConfig, llm: {
          ...coderConfig.llm, model: coderResult.mode === 'llm' ? coderResult.model : coderConfig.llm.model,
        } };
      let review;
      try {
        review = await liveLog.seat('reviewer', sessions.reviewer, reviewConfig, (onEvent) => runReviewer({
          worktree: worktreePath, repoRoot, config: reviewConfig,
          coderResult, fetchImpl, env, vault, onEvent, askKind, retryCommand, signal, previousFindings, priorWaveFiles,
        }));
      } catch (error) {
        const failure = independent ? routeFailure(error) : null;
        if (failure) {
          // The independent reviewer's endpoint failed: record it as route evidence and try the next
          // non-coder profile without touching the coder's route.
          routeAttempts.push({ seat: 'reviewer', profile: independent.profile.id, ...failure });
          const failedProfile = independent.profile.id;
          independent = await independentReviewerRoute(incompleteReviewers);
          log(`Route recovery: seat=reviewer profile=${failedProfile} endpoint failed; ` + (independent
            ? `continuing with profile=${independent.profile.id} model=${independent.profile.model}.`
            : `no other independent profile remains; reviewing with the coder's model.`));
          continue;
        }
        if (!await recoverRoute('reviewer', error)) throw error;
        reviewerRoute = route;
        continue;
      }
      if (independent && review.queried && !review.completed && incompleteReviewers.length === 0) {
        // One reviewer model failing to format its report must not strand tested coder work: retry once on
        // the next independent profile. Without one, the incomplete review stands.
        const incompleteProfile = independent.profile.id;
        incompleteReviewers.push(incompleteProfile);
        const next = await independentReviewerRoute(incompleteReviewers);
        if (next) {
          log(`Reviewer profile=${incompleteProfile} returned an incomplete review; ` +
            `retrying with profile=${next.profile.id} model=${next.profile.model}.`);
          // The discarded attempt still spent real tokens; record it so seat metrics do not undercount.
          await recordSeat(sessions.reviewer, buildRun({
            config: reviewConfig, response: review.response, session: sessions.reviewer, task: prepared.task, env,
          }), undefined, reviewConfig);
          await removeIncompleteReview(review.reviewPath);
          independent = next;
          continue;
        }
        log(`Reviewer profile=${incompleteProfile} returned an incomplete review; no other independent profile remains.`);
      }
      const reviewerRun = review.queried ? buildRun({
        config: reviewConfig, response: review.response, session: sessions.reviewer,
        task: prepared.task, env,
      }) : null;
      await recordSeat(sessions.reviewer, reviewerRun, undefined, reviewConfig);
      return { review, reviewerRun };
    }
  };
  let priorWaveFiles = [];
  if (!prepared.local && (issueWave(prepared.issue) ?? 1) > 1) {
    try {
      const repository = githubRepository((await issueCommand('git', ['remote', 'get-url', 'origin'], prepared.repoRoot)).trim());
      priorWaveFiles = await earlierWaveFiles({ issue: prepared.issue, repository, worktree: worktreePath,
        cwd: prepared.repoRoot, runCommand: issueCommand });
    } catch { priorWaveFiles = []; }
    if (priorWaveFiles.length) log(`Earlier waves delivered: ${priorWaveFiles.join(', ')}; the coder is told to reuse them.`);
  }
  const coderSeat = (priorFeedback = planner.feedback?.context,
    { initialBaseline, initialScopeFiles, initialRepairFiles, continuation, priorWrites } = {}) => {
    assertSeatCovers(recipeCoder, {
      ...(recipeCoder.max_difficulty === undefined ? {} : { difficulty: readTaskMetadata(planner.task).difficulty }),
      ...(recipeCoder.skills === undefined ? {} : { skills: taskSkillNames(planner.task) }),
    });
    return liveLog.seat('coder', sessions.coder, coderConfig, (onEvent) => runCoder({
      worktree: worktreePath, repoRoot, config: coderConfig, task: prepared.task, session: sessions.coder,
      fetchImpl, env, vault, runTestCommand, priorFeedback, onEvent, askKind, retryCommand, signal, steeringControl,
      initialBaseline, initialScopeFiles, initialRepairFiles, continuation, priorWaveFiles, priorWrites,
    }));
  };
  let result;
  let coderBaseline;
  let coderScopeFiles = [];
  let coderRepairFiles = [];
  // Files earlier coder contexts in this run changed; a fresh context must not excuse their failures as pre-existing.
  let coderWrites = [];
  if (planner.reused) {
    const restored = await restoreRunScope(worktreePath, prepared.task, (args) => git(worktreePath, args, commandEnv),
      taskFilesAllowed(planner.task)).catch(() => null);
    if (restored && restored.repairFiles.length + restored.scopeFiles.length) {
      ({ repairFiles: coderRepairFiles, scopeFiles: coderScopeFiles } = restored);
      log(`Restored recorded scope from earlier runs: ${[...restored.repairFiles, ...restored.scopeFiles].join(', ')}.`);
    }
  }
  let continuation;
  let reviewFindings = [];
  if (planner.reused) {
    const passed = (text) => /^Verdict: pass\s*$/m.test(text ?? '');
    const carried = previousReviewFindings(previousReview) ?? (passed(previousReview) ? undefined :
      previousReviewFindings(await latestArchivedReview(worktreePath, { task: prepared.task,
        git: (args) => git(worktreePath, args, commandEnv),
        accept: (text) => previousReviewFindings(text) !== undefined, supersedes: passed }).catch(() => null)));
    if (carried) {
      continuation = reviewRepairContinuation({ ...carried,
        heading: 'Previous run: the reviewer failed the last result for this same TASK' });
      reviewFindings = carried.reasons;
    }
  }
  if (continuation) log('Carrying the previous failed review findings into the coder context.');
  const perspectiveAttempts = [];
  const rescopes = [];
  let coderRun;
  let review;
  let reviewerRun;
  for (;;) {
  for (;;) {
    try {
      result = await coderSeat(undefined, { initialBaseline: coderBaseline, initialScopeFiles: coderScopeFiles,
        initialRepairFiles: coderRepairFiles, continuation, priorWrites: coderWrites });
      break;
    } catch (error) {
      if (error instanceof Error && error.result) {
        await recordSeat(sessions.coder, error.result.run, error.result.excellence, coderConfig);
      }
      const failedProfile = route?.profile.id;
      if (await recoverRoute('coder', error)) {
        coderConfig = buildCoderConfig(activeConfig.llm.model);
        continuation =
          `The previous coder attempt on fleet profile ${failedProfile} stopped because of an endpoint route failure, ` +
          'not a task failure. Its edits remain in the worktree: inspect them, keep what is correct, and continue ' +
          'to a verified result with a different approach rather than repeating the same steps.';
      } else if (rescopeBudget(error, coderConfig.seat.scope_expansion ?? 3, rescopes.length)) {
        // The plan is a pre-read estimate: a coder blocked only by the expansion limit gets a larger budget,
        // not a failed run. Protected paths stay denied and the reviewer still judges every expanded file.
        const previous = coderConfig.seat.scope_expansion ?? 3;
        scopeBudget = rescopeBudget(error, previous, rescopes.length);
        coderConfig = buildCoderConfig(coderConfig.llm.model);
        const needed = error.result.scopeBlocked;
        rescopes.push({ from: previous, to: scopeBudget, files: needed });
        log(`Re-scope ${rescopes.length} of ${maxRescopes}: coder needed ${needed.join(', ')} beyond its ` +
          `${previous}-file expansion budget; continuing with ${scopeBudget} files in a fresh context.`);
        onRunEvent?.({ type: 'rescope', attempt: rescopes.length, from: previous, to: scopeBudget, files: needed });
        continuation = rescopeContinuation({ previous, budget: scopeBudget, files: needed,
          changedFiles: error.result?.excellence?.files ?? [] });
      } else {
        // An exhausted repair or turn budget is evidence that this context is stuck, not that the task is
        // impossible: retry once or twice with a fresh context (and a different profile when one is eligible).
        const stuck = coderStuckReason(error);
        const failCount = error.result?.failCount;
        if (!stuck || perspectiveAttempts.length >= maxPerspectiveEscalations) throw error;
        const previousModel = coderConfig.llm.model;
        const alternate = autoModel && route ? await selectAutoRoute([...new Set([
          ...routeAttempts.map((attempt) => attempt.profile), ...perspectiveAttempts.map((attempt) => attempt.profile),
          failedProfile,
        ])]) : null;
        if (alternate) coderConfig = buildCoderConfig(activeConfig.llm.model);
        perspectiveAttempts.push({ profile: failedProfile ?? null, model: previousModel, reason: stuck, failCount });
        log(`Perspective escalation ${perspectiveAttempts.length} of ${maxPerspectiveEscalations}: coder ${stuck}` +
          (Number.isSafeInteger(failCount) ? ` with ${failCount} failing test(s)` : '') + '; ' +
          (alternate ? `continuing with profile=${alternate.profile.id} model=${alternate.profile.model} in a fresh context.`
            : `continuing with model=${coderConfig.llm.model} in a fresh context.`));
        onRunEvent?.({ type: 'perspective-escalation', attempt: perspectiveAttempts.length, reason: stuck,
          model: coderConfig.llm.model });
        continuation = perspectiveContinuation({ attempt: perspectiveAttempts.length, reason: stuck,
          evidence: redactEvidence(error.message, { env, apiKeyEnv: config.llm.api_key_env }),
          changedFiles: error.result?.excellence?.files ?? [], history: perspectiveAttempts,
          sameModel: coderConfig.llm.model === previousModel });
      }
      coderBaseline ??= error.result?.baseline;
      coderScopeFiles = [...new Set([...coderScopeFiles, ...(error.result?.scopeFiles ?? [])])];
      coderRepairFiles = [...new Set([...coderRepairFiles, ...(error.result?.repairFiles ?? [])])];
      coderWrites = [...new Set([...coderWrites, ...(error.result?.excellence?.files ?? [])])];
      await archiveRunArtifacts(worktreePath, {
        task: prepared.task, git: (args) => git(worktreePath, args, commandEnv),
        preserve: ['RECIPE.yml', 'TASK.md', 'ESTIMATE.md'],
      });
    }
  }
  coderRun = result.run;
  await recordSeat(sessions.coder, coderRun, result.excellence, coderConfig);
  ({ review, reviewerRun } = await reviewSeat(result, reviewFindings));
  if (review.verdict !== 'fail' || !review.completed || skipReview || result.mode !== 'llm' ||
      reviewRepairs.length >= maxReviewRepairs) break;
  const unmetChecks = review.unmetChecks ?? [];
  const previousUnmet = reviewRepairs.at(-1)?.unmetChecks;
  // Repeating a repair that left the same checks unmet is the same move twice; switch perspective instead.
  const stalled = previousUnmet !== undefined && unmetChecks.length > 0 &&
    unmetChecks.every((id) => previousUnmet.includes(id));
  const failedProfile = route?.profile.id;
  const alternate = stalled && autoModel && route ? await selectAutoRoute([...new Set([
    ...routeAttempts.map((attempt) => attempt.profile), ...perspectiveAttempts.map((attempt) => attempt.profile),
    ...reviewRepairs.map((repair) => repair.profile), failedProfile,
  ])]) : null;
  if (alternate) coderConfig = buildCoderConfig(activeConfig.llm.model);
  reviewRepairs.push({ profile: failedProfile ?? null, model: result.model ?? coderConfig.llm.model, unmetChecks });
  log(`Review repair ${reviewRepairs.length} of ${maxReviewRepairs}: reviewer failed ` +
    (unmetChecks.length ? `checks ${unmetChecks.join(', ')}` : 'the result') + '; ' +
    (alternate ? `switching to profile=${alternate.profile.id} model=${alternate.profile.model}`
      : `continuing with model=${coderConfig.llm.model}`) + ' in a fresh coder context with the findings.');
  onRunEvent?.({ type: 'review-repair', attempt: reviewRepairs.length, unmetChecks, model: coderConfig.llm.model });
  continuation = reviewRepairContinuation({ round: reviewRepairs.length,
    reasons: review.reasons, unmetChecks, stalled, changedFiles: result.excellence?.files ?? [] });
  reviewFindings = review.reasons ?? [];
  coderBaseline ??= result.baseline;
  coderScopeFiles = [...new Set([...coderScopeFiles, ...(result.scopeFiles ?? [])])];
  coderRepairFiles = [...new Set([...coderRepairFiles, ...(result.repairFiles ?? [])])];
  coderWrites = [...new Set([...coderWrites, ...(result.excellence?.files ?? [])])];
  await archiveRunArtifacts(worktreePath, {
    task: prepared.task, git: (args) => git(worktreePath, args, commandEnv),
    preserve: ['RECIPE.yml', 'TASK.md', 'ESTIMATE.md'],
  });
  }
  await ensureUnchanged(planner.recipePath, planner.recipe);
  await ensureUnchanged(planner.taskPath, planner.task);
  await ensureUnchanged(planner.estimatePath, planner.estimate);
  const runs = { planner: plannerRun, coder: coderRun, reviewer: reviewerRun };
  const model = result.mode === 'llm' ? coderRun.metrics.model : null;
  const publishMessage = model ? buildPublishMessage({
    subject: prepared.local ? 'feat: local ask' : `feat: issue ${prepared.issue.number}`,
    model, issueNumber: prepared.issue.number,
    summary: redactEvidence(result.summary, { env, apiKeyEnv: config.llm.api_key_env }),
    testsSkipped: result.testsSkipped,
    scopeFiles: result.scopeFiles ?? [],
    seats: `planner, coder, reviewer (${skipReview ? 'gate bypassed with --skip-review'
      : !isReviewRequired(config) ? 'gate not required by configuration' : review.verdict})`,
  }) : null;
  const command = model && config.publish?.enabled !== false && (review.verdict === 'pass' || reviewBypass)
    ? formatPublishCommand({ message: publishMessage, model }) : null;
  log(`Worktree: ${worktreePath}\nAssignment: ${prepared.assignmentPath}\n` +
    `Live log: ${liveLog.path}\n` +
    `RECIPE: ${planner.recipePath}\nTASK: ${planner.taskPath}\nESTIMATE: ${planner.estimatePath}\n` +
    `RESULT: ${result.resultPath}\nREVIEW: ${review.reviewPath} (${review.verdict})\n` +
    (review.verdict === 'fail' ? `Review failure: ${redactEvidence(review.reasons.join('; '), {
      env, apiKeyEnv: config.llm.api_key_env,
    })}\n` : '') +
    `Planner session: ${sessions.planner}\n` +
    (plannerRun ? `AI-Run: ${plannerRun.line}\n` : '') +
    `Coder session: ${sessions.coder}\n` +
    (coderRun ? `AI-Run: ${coderRun.line}\n` +
      `For manual publication, set these variables (empty values clear inherited fields):\n` +
      formatPublishEnvironment(coderRun.env)
      : 'Stub run: no AI-Run metadata and no code to publish.\n') +
    `Reviewer session: ${sessions.reviewer}\n` +
    (reviewerRun ? `AI-Run: ${reviewerRun.line}\n` : '') +
    (command ? (review.verdict === 'fail'
      ? `WARNING: review failed; publication is permitted only because ${skipReview
        ? '--skip-review explicitly bypasses the gate' : 'the configured review gate is disabled'}. These changes are not approved.\n`
      : '') + `From the worktree root, publish only after reviewing changes:\n` +
      formatPublishCommand({
        message: prepared.local ? 'feat: local ask' : `feat: issue ${prepared.issue.number}`,
        model,
      }) + '\n/publish sends the reviewed summary; do not paste the model summary into --message.'
      : config.publish?.enabled === false
        ? 'Publication unavailable: publishing is disabled by publish.enabled.'
        : model
        ? 'Publication unavailable: REVIEW.md failed; rerun the reviewer or explicitly use --skip-review.'
        : 'Publication unavailable: set model and complete a configured coder run with passing checks.'));
  if (review.verdict === 'pass') {
    const stat = await reviewedDiffStat(worktreePath, result.excellence.files, commandEnv);
    log(`Reviewed worktree: ${worktreePath}\ngit diff --stat:\n` +
      redactEvidence(stat || '(no application diff)', { env, apiKeyEnv: config.llm.api_key_env }) +
      `\nAfter merge, human AI-Eval (replace M with actual minutes):\n${humanEvalHint(sessions.coder)}`);
  }

  const completed = {
    ...prepared, askKind, classification, recipePath: planner.recipePath, taskPath: planner.taskPath,
    planner, result, review, sessions, runs, run: coderRun, command, autoRecommendation, route, routeAttempts,
    perspectiveAttempts, rescopes, reviewRepairs, archivePath, delivery,
    failed: false,
    logPath: liveLog.path, logSession: liveLog.session,
  };
  if (publish && !isolated) {
    await publishCompleted(completed, coderConfig, publishMessage);
  }
  return completed;
  };
  const routeAttemptsForRun = routeAttempts;
  const recoverRunRoute = recoverRoute;
  const selectRunRoute = selectAutoRoute;
  const currentRunRoute = () => ({ activeConfig, route });
  if (attempts === 1) return executePlanned();
  return runPlanAttempts({
    count: attempts, prepared, planner, config, env, signal,
    command: issueCommand,
    cleanupCommand: (program, args, root) => runCommand ? runCommand(program, args, root) : git(root, args, commandEnv),
    choose: (excludedProfileIds) => chooseFleetRoute(excludedProfileIds),
    execute: async (attemptPrepared, attemptPlanner, selected) => {
      const attemptSessions = { planner: sessions.planner, coder: attemptPrepared.session,
        reviewer: `${attemptPrepared.session.replace(/-coder$/, '')}-reviewer` };
      const selectedConfig = { ...withFleetProfile(config, selected.profile),
        llm: Object.freeze(routeLlm(selected)) };
      const attemptLog = await createRunLog({ repoRoot: prepared.repoRoot,
        session: attemptPrepared.session, env, apiKeyEnv: selectedConfig.llm.api_key_env,
        errorOutput, now, debug, issue: prepared.issue.number, observe: onRunEvent });
      return executePlanned({ prepared: attemptPrepared, planner: attemptPlanner,
        activeConfig: selectedConfig, route: selected, sessions: attemptSessions, liveLog: attemptLog, isolated: true });
    },
    publish: publish ? async (winner, selected) => publishCompleted(winner, withFleetProfile(config, selected.profile),
      buildPublishMessage({ subject: `feat: issue ${prepared.issue.number}`, model: winner.run.metrics.model,
        issueNumber: prepared.issue.number,
        summary: redactEvidence(winner.result.summary, { env, apiKeyEnv: config.llm.api_key_env }),
        testsSkipped: winner.result.testsSkipped, scopeFiles: winner.result.scopeFiles ?? [],
        seats: 'planner, coder, reviewer (pass)' }) + attemptSummary(winner)) : undefined,
    log,
  });

  async function publishCompleted(completed, coderConfig, publishMessage) {
    const { worktreePath, runs: { coder: coderRun } } = completed;
    const { contractsPath, publishEnv, model: publishModel } = await prepareBuiltinPublication(completed, {
      cwd, config: coderConfig, env, skipReview, signal,
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
        const publication = await issueCommenter({
          issue: prepared.issue, pullNumber: merged, model: coderRun.env.AI_MODEL,
          runLine: coderRun.line, run: coderRun,
          repoRoot: prepared.repoRoot, cwd, env,
        });
        await recordDeliveryPublication(completed, publication);
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
    const publication = await issueCommenter({
      issue: prepared.issue, pullNumber, model: coderRun.env.AI_MODEL,
      runLine: coderRun.line, run: coderRun,
      repoRoot: prepared.repoRoot, cwd, env,
    });
    await recordDeliveryPublication(completed, publication);
  }
}
