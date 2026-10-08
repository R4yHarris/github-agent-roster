import { execFileSync } from 'node:child_process';
import { lstatSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { Writable } from 'node:stream';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { isReviewRequired, loadConfig, requirePublicationEnabled, setConfigValue } from './lib/config.mjs';
import { recordEvaluation } from './lib/eval.mjs';
import { inferTaskClass, parseRecommendationArgs, repositoryRoot } from './lib/learn.mjs';
import { formatMetrics, loadAvailableMetrics, loadMetrics, summarizeMetrics } from './lib/metrics.mjs';
import { formatFailureProposals, listFailureProposals, proposeRecurringFailures } from './lib/failure-proposals.mjs';
import { resolveContractsPath, resolveProjectRoot } from './lib/paths.mjs';
import { formatRoute, routeTask } from './lib/route.mjs';
import {
  buildPublishMessage, formatPublishCommand, formatPublishEnvironment, parsePublishArgs, publicationTask,
} from './lib/publication.mjs';
import { redactEvidence } from './runtime/excellence.mjs';
import { isLlmTimeout } from './llm/request.mjs';
import { formatStatus, readStatus } from './lib/status.mjs';
import { createFileVault, validateSecretName } from './vault/file.mjs';
import { buildPublishEnv, resolvePublishModel } from './metrics/run.mjs';
import { humanEvalHint } from './lib/seat-publication.mjs';
import { readIssueLogs } from './lib/run-log.mjs';
import { createDebugLog } from './lib/debug-log.mjs';
import { createTray } from './shell/tray.mjs';
import { createTranscript } from './shell/transcript.mjs';
import { createEventSink, createShellPainter } from './shell/events.mjs';
import { canonicalCommand, completeCommand, formatHelp } from './shell/commands.mjs';
import { createHistory, safeHistoryLine } from './shell/history.mjs';
import { isRunCancelled, RunCancelledError } from './runtime/cancel.mjs';
import { formatSessionStatus } from './shell/status.mjs';
import { getFleetProfile, loadFleet, withFleetProfile } from './lib/fleet.mjs';
import { runFleet } from './lib/fleet-cli.mjs';
import { probeModelDetails } from './onboard/wizard.mjs';
import { contextMaxForModel } from './llm/window.mjs';
import { splitArguments } from './lib/arguments.mjs';
import { formatIssueSummary, listOpenIssues, readDiffNames } from './lib/board.mjs';
import { checkDoctor, formatDoctor, warmDoctor } from './lib/doctor.mjs';
import { configSetting, privateConfigPath, publicConfig } from './shell/settings.mjs';
import { parseShellEvaluationArgs } from './shell/evaluation.mjs';
import { listCheckpoints, rewindCheckpoint } from './lib/checkpoints.mjs';
import { readPlannerTask } from './seats/planner.mjs';
import { taskAndRepairFiles } from './runtime/tools.mjs';
import { taskFilesAllowed } from './planner/task.mjs';
import { listLocalRuns, readLocalRun, recapRun } from './lib/local-runs.mjs';
import { askSideQuestion } from './lib/side-question.mjs';
import { formatContext } from './shell/context.mjs';
import { formatUsage } from './shell/usage.mjs';
import { listIssueWorktrees } from './lib/worktrees.mjs';
import { runOnlyReview } from './lib/review.mjs';
import { waveBoard } from './lib/waves.mjs';
import { writeRepoMap } from './lib/repo-map.mjs';
import { createSteeringControl } from './runtime/steering.mjs';
import { remediationTask, selectRemediationProfile } from './runtime/remediation.mjs';

const rosterRoot = fileURLToPath(new URL('../', import.meta.url));
const unknownCommand = 'Unknown command. /help lists commands.\n';

class ChecksPermissionError extends Error {}

async function publishWithContracts({ contractsPath, cwd, env, message, model, output, errorOutput }) {
  const { main } = await import(pathToFileURL(join(contractsPath, 'scripts', 'agent-pr.mjs')).href);
  let errorText = '';
  let successText = '';
  const stdout = {
    write(text) {
      successText += String(text);
      output.write(text);
    },
  };
  const stderr = {
    write(text) {
      errorText += String(text);
      errorOutput.write(text);
    },
  };
  const code = await main(['--message', message, '--model', model, '--merge-when-green'], {
    cwd, env, stdout, stderr,
  });
  const { mergedPullNumber, mergedPullNumberFromFailure } = await import('./lib/issue-board.mjs');
  if (code === 0) {
    return { mergedPullRequest: /^Merged PR #/m.test(successText)
      ? mergedPullNumber(successText) : null };
  }
  const merged = mergedPullNumberFromFailure(errorText);
  if (merged !== null) {
    const error = new Error(`PR #${merged} merged, but local publisher cleanup failed; inspect the worktree.`);
    error.mergedPullRequest = merged;
    throw error;
  }
  if (errorText.includes('HTTP 422')) {
    throw new ChecksPermissionError('Checks permission is not accepted on the installation.');
  }
  throw new Error('App publication failed; see the publisher error above.');
}

// Seat, LLM, reviewer, and issue-board modules load on first use so /help, /status, and /quit start fast.
const lazy = (specifier, name) => async (...args) => (await import(specifier))[name](...args);
const requirePassingReview = lazy('./seats/reviewer.mjs', 'requirePassingReview');

const defaultServices = {
  runBuiltinAsk: lazy('./lib/builtin.mjs', 'runBuiltinAsk'),
  runBuiltinIssue: lazy('./lib/builtin.mjs', 'runBuiltinIssue'),
  prepareBuiltinPublication: lazy('./lib/builtin.mjs', 'prepareBuiltinPublication'),
  recordEvaluation, repositoryRoot, loadMetrics,
  summarizeMetrics, formatMetrics, loadAvailableMetrics, routeTask, formatRoute,
  formatFailureProposals, listFailureProposals, proposeRecurringFailures,
  resolveContractsPath, createFileVault,
  validateSecretName, readStatus, formatStatus, setConfigValue,
  publicationTask,
  readIssueLogs,
  getFleetProfile, loadFleet, withFleetProfile, runFleet, probeModelDetails,
  formatIssueSummary, listOpenIssues, readDiffNames,
  checkDoctor, formatDoctor, warmDoctor, privateConfigPath, publicConfig,
  listCheckpoints, rewindCheckpoint, readPlannerTask,
  listLocalRuns, readLocalRun, recapRun,
  askSideQuestion,
  listIssueWorktrees,
  runOnlyReview,
  waveBoard,
  writeRepoMap,
  issueCommenter: lazy('./lib/issue-board.mjs', 'commentMergedIssue'),
  publisher: publishWithContracts,
  repositoryBranch(cwd) {
    const root = resolveProjectRoot(cwd);
    if (!lstatSync(join(root, '.git'), { throwIfNoEntry: false })) return '-';
    return execFileSync('git', ['rev-parse', '--abbrev-ref', 'HEAD'], {
      cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
    }).trim();
  },
};

function conventionalSubject(value) {
  if (!/^[a-z]+(?:\([A-Za-z0-9_.-]+\))?!?: [^\r\n]+$/.test(value) ||
      /["\\$`]/.test(value) || value.length > 200) {
    throw new TypeError('Use /publish <conventional subject> without shell metacharacters.');
  }
  return value;
}

export function createDispatcher({
  cwd = process.cwd(),
  repoRoot = rosterRoot,
  config = loadConfig({ repoRoot, cwd }),
  env = process.env,
  output = process.stdout,
  errorOutput = process.stderr,
  services = {},
  debug = createDebugLog({ env }),
  onStateChange = () => {},
  onUiAction = () => {},
  input = process.stdin,
  askInput,
} = {}) {
  const api = { ...defaultServices, ...services };
  const state = { lastAsk: null, lastRun: null, pendingSecret: null, published: false, config, debug,
    lastRequest: null, pendingConfirm: null,
    routeNext: false, fleetProfileId: null, pendingQuestion: false,
    sideController: null,
    lastMeasuredSeat: null,
    steeringControl: null, queuedInput: [], askQueue: [],
    statusbar: true, controller: null, history: [], issueCache: new Map(), display: {
      issue: null, branch: api.repositoryBranch(cwd), seat: config.seat.id, state: 'idle', busy: false,
      model: config.llm.model, host: config.llm.base_url ? new URL(config.llm.base_url).host : '',
      effort: config.llm.effort, contextUsed: undefined, contextMax: config.llm.context_max, startedAt: null,
      lastFinishReason: null, lastTestName: null, review: null,
      outputTokens: undefined, toolCount: undefined, thinking: undefined, maxTokens: undefined,
    }, transcript: null };
  const notify = () => onStateChange(state);
  const contextWindows = new Map();
  const windowKey = (llm = state.config.llm) => `${llm.base_url ?? ''}|${llm.model ?? ''}`;
  function applyContextWindow() {
    if (state.display.model && state.display.model !== state.config.llm.model) return;
    const reported = contextWindows.get(windowKey());
    if (reported !== undefined) state.display.contextMax = reported;
  }
  async function ensureContextWindow() {
    const llm = state.config.llm;
    const key = windowKey(llm);
    if (!llm.base_url || contextWindows.has(key)) {
      applyContextWindow();
      return;
    }
    contextWindows.set(key, undefined);
    try {
      const models = await api.probeModelDetails(llm.base_url, { env, apiKeyEnv: llm.api_key_env });
      const reported = contextMaxForModel(models, llm.model, undefined);
      if (reported !== undefined) contextWindows.set(key, reported);
    } catch {
      // An endpoint that does not report a window keeps the configured context max as the denominator.
    }
    applyContextWindow();
    notify();
  }
  const sink = createEventSink({ issue: () => state.display.issue,
    emit: createShellPainter({ transcript: () => state.transcript, display: state.display, notify }) });
  const receiveEvent = (event) => {
    const display = state.display;
    if (event.wave !== undefined && Number.isSafeInteger(event.issue)) {
      const rows = display.waves ??= [];
      let row = rows.find((item) => item.issue === event.issue);
      if (!row) {
        row = { issue: event.issue, wave: event.wave, state: 'running' };
        rows.push(row);
      }
      if (event.type === 'wave-end') {
        row.state = event.state;
      } else if (event.type === 'seat-start') {
        Object.assign(row, { seat: event.seat, model: event.model,
          state: { planner: 'planning', coder: 'drafting', reviewer: 'reviewing' }[event.seat] });
      } else if (event.type === 'route' || event.type === 'seat-measurement') {
        if (event.model) row.model = event.model;
      } else if (event.type === 'seat-error') row.state = 'failed';
      else if (event.type === 'seat-end' && event.verdict) row.state = event.verdict === 'pass' ? 'passed' : 'failed';
      else if (event.type === 'tool' && event.name === 'run_test') row.state = 'testing';
      notify();
      return;
    }
    sink.receive(event);
    if (event.type === 'seat-measurement') {
      state.lastMeasuredSeat = Object.fromEntries(['seat', 'provider', 'model', 'effort', 'input', 'output',
        'contextMax', 'finishReason', 'packBudgetChars', 'priorFeedbackIncluded'].map((key) => [key, event[key]]));
      applyContextWindow();
      return;
    }
    if (event.type === 'route') {
      Object.assign(display, { model: event.model, host: event.host, contextMax: event.contextMax,
        hardware: event.hardware, contextUsed: undefined });
      notify();
      return;
    }
    display.seat = event.seat;
    if (['finish-reason', 'completion'].includes(event.type)) display.lastFinishReason = event.reason;
    if (event.type === 'tool') display.toolCount = (display.toolCount ?? 0) + 1;
    if (event.type === 'checklist') display.checklist = `${event.done}/${event.total}`;
    if (event.type === 'tool' && event.name === 'run_test') display.lastTestName = 'node --test';
    if (event.type === 'seat-start') {
      display.state = { planner: 'planning', coder: 'drafting', reviewer: 'reviewing' }[event.seat];
      for (const [key, value] of [['model', event.model], ['host', event.host], ['effort', event.effort],
        ['contextMax', event.contextMax]]) if (value !== undefined) display[key] = value;
      Object.assign(display, { contextUsed: undefined, outputTokens: undefined, toolCount: 0, checklist: undefined });
    } else if (event.type === 'tool' && event.name === 'run_test') display.state = 'testing';
    else if (event.type === 'http' && event.phase === 'start') {
      display.state = { planner: 'planning', coder: 'drafting', reviewer: 'reviewing' }[event.seat];
      if (event.effort !== undefined) display.effort = event.effort;
      if (event.thinking !== undefined) display.thinking = event.thinking;
      if (event.maxTokens !== undefined) display.maxTokens = event.maxTokens;
    } else if (event.type === 'seat-error') display.state = 'failed';
    else if (event.type === 'seat-end') {
      if (event.model) display.model = event.model;
      display.contextUsed = event.contextUsed;
      if (event.outputTokens !== undefined) display.outputTokens = event.outputTokens;
      if (event.verdict) display.state = event.verdict === 'pass' ? 'passed' : 'failed';
      if (event.verdict) display.review = event.verdict;
    }
    applyContextWindow();
    notify();
  };
  const currentRoot = () => state.lastRun?.repoRoot ?? api.repositoryRoot(cwd);
  const metrics = (ref) => api.loadMetrics({
    contractsPath: api.resolveContractsPath({ repoRoot, cwd, env }),
    cwd: currentRoot(),
    ...(ref ? { ref } : {}),
  });
  const host = (active = state.config) => active.llm.base_url ? new URL(active.llm.base_url).host : '-';
  const safeWrite = (value) => output.write(redactEvidence(value, { env, apiKeyEnv: state.config.llm.api_key_env }));
  const syncModelDisplay = () => {
    Object.assign(state.display, { model: state.config.llm.model, host: host(), effort: state.config.llm.effort,
      contextMax: state.config.llm.context_max, contextUsed: undefined });
    applyContextWindow();
    notify();
  };
  async function selectModel(value, save = false) {
    const model = value === 'clear' ? '' : resolvePublishModel({ env: { AI_MODEL: value } });
    if (model === 'builtin-stub') throw new TypeError('Use an actual served model ID, not builtin-stub.');
    if (redactEvidence(model, { env, apiKeyEnv: state.config.llm.api_key_env }) !== model) {
      throw new TypeError('Use a public model ID, not a secret.');
    }
    if (save) await api.setConfigValue('model', model, { repoRoot, cwd });
    state.config = { ...state.config, llm: { ...state.config.llm, model } };
    state.routeNext = !model;
    syncModelDisplay();
    safeWrite(`Model: ${model || '(unset)'}\nHost: ${host()}\n`);
  }

  function queueAsk(text) {
    const ask = text.trim();
    if (!ask || ask.length > 4096 || redactEvidence(ask, { env, apiKeyEnv: state.config.llm.api_key_env }) !== ask) {
      throw new TypeError('Queued Ask is empty, longer than 4096 characters, or contains secret material.');
    }
    if (state.askQueue.length === 8) throw new TypeError('Ask queue is full (8). Use /queue drop N or /queue clear.');
    state.askQueue.push(ask);
    output.write(`Queued Ask ${state.askQueue.length}: ${ask.slice(0, 120)}\n`);
    notify();
  }

  function listAskQueue() {
    if (!state.askQueue.length) {
      output.write('No Asks queued.\n');
      return;
    }
    output.write(state.askQueue.map((ask, index) => `${index + 1}. ${ask}`).join('\n') + '\n');
  }

  function queueInput(line) {
    const text = line.trim();
    const combined = [...state.queuedInput, text].join('\n');
    if (state.queuedInput.length === 8 || combined.length > 4096 ||
        redactEvidence(combined, { env, apiKeyEnv: state.config.llm.api_key_env }) !== combined) {
      throw new TypeError('Queued input limit or secret-material guard refused the line.');
    }
    state.queuedInput.push(text);
    output.write('Input queued. Use /steer TEXT to send it to the drafting coder.\n');
  }

  function discardQueued(signal) {
    if (state.queuedInput.length && !signal.aborted) output.write('Queued input was not sent to the coder.\n');
    state.queuedInput = [];
  }

  function cancel() {
    if (state.sideController && !state.sideController.signal.aborted) {
      state.sideController.abort(new RunCancelledError());
      if (state.controller && !state.controller.signal.aborted) state.controller.abort(new RunCancelledError());
      return true;
    }
    if (state.pendingConfirm !== null) {
      state.pendingConfirm = null;
      state.display.state = 'idle';
      state.display.mode = null;
      notify();
      output.write('Run cancelled.\n');
      return true;
    }
    if (state.controller === null || state.controller.signal.aborted) return false;
    state.controller.abort(new RunCancelledError());
    return true;
  }

  async function executeRun(run, options = {}, request) {
    if (state.controller !== null) throw new Error('A seat is already running; cancel it before starting another.');
    const controller = new AbortController();
    state.controller = controller;
    state.steeringControl = options.parallel > 1 || options.attempts > 1
      ? null : createSteeringControl({ signal: controller.signal });
    state.display.waves = [];
    state.queuedInput = [];
    state.pendingConfirm = null;
    if (request) state.lastRequest = request;
    state.lastRun = { planningOnly: true, failed: true, askKind: 'clarify', command: null };
    state.published = false;
    Object.assign(state.display, { issue: options.issue ?? null,
      branch: options.issue ? `issue-${options.issue}` : state.display.branch,
      seat: 'planner', state: 'planning', busy: true, startedAt: Date.now(), contextUsed: undefined,
      lastFinishReason: null, lastTestName: null, review: null });
    state.display.mode = options.planMode ? 'plan' : null;
    notify();
    // The window probe repaints the rail when it answers; it never delays the seat.
    ensureContextWindow().catch(() => {});
    try {
      state.lastRun = await run({
        cwd, repoRoot, config: state.config, env, publish: false, debug: state.debug, ...options,
        log: (message) => output.write(`${message}\n`), errorOutput,
        onRunEvent: receiveEvent,
        signal: controller.signal,
        steeringControl: state.steeringControl,
        onPrepared(prepared) {
          if (request) request.prepared = prepared;
          state.lastRun = { ...prepared, planningOnly: true, failed: true, askKind: 'clarify', command: null };
          state.display.branch = prepared.task;
          notify();
        },
      });
    } catch (error) {
      if (isLlmTimeout(error)) {
        state.lastRun = { planningOnly: true, failed: true, timedOut: true, command: null };
      }
      state.display.state = isRunCancelled(error) ? 'idle' : 'failed';
      throw error;
    } finally {
      discardQueued(controller.signal);
      state.steeringControl?.dispose();
      state.steeringControl = null;
      state.display.busy = false;
      state.controller = null;
      notify();
    }
    if (state.lastRun.task) state.display.branch = state.lastRun.task;
    if (request) request.prepared = state.lastRun;
    if (state.lastRun.confirmedPause && request) state.pendingConfirm = request;
    if (state.lastRun.planMode && state.lastRun.askKind === 'slice' && request) state.pendingConfirm = request;
    state.display.state = state.lastRun.review
      ? state.lastRun.review.verdict === 'pass' ? 'passed' : 'failed'
      : state.lastRun.failed ? 'failed' : 'idle';
    if (state.lastRun.planMode) state.display.state = 'planning';
    state.display.review = state.lastRun.review?.verdict ?? null;
    if (state.lastRun.issue?.number) state.issueCache.set(state.lastRun.issue.number, {
      issue: state.lastRun.issue, branch: state.lastRun.task, openPr: undefined, worktreePath: state.lastRun.worktreePath,
      display: { ...state.display },
    });
    notify();
    output.write(state.lastRun.askKind === 'clarify'
      ? `${state.lastRun.clarification}\n`
      : state.lastRun.failed
      ? 'Planning failed; stubs are unverified. Coder, tests, reviewer, and publication did not run. Check the planner diagnostic, then retry.\n'
      : state.lastRun.planMode
      ? 'Plan ready. Press Enter to accept and continue, or /stop to keep the plan without coding.\n'
      : state.lastRun.parallelRun
      ? 'Parallel children finished; each reviewed worktree has its own App SDK handoff. Use /status for child outcomes.\n'
      : state.lastRun.planPath
      ? 'PLAN ready; review the child drafts and run bounded slices. No coder ran.\n'
      : state.lastRun.planningOnly
      ? 'Paused by --confirm. Press Enter to continue, or /stop to cancel.\n'
      : state.lastRun.review?.verdict === 'fail'
      ? `Review failed: ${redactEvidence((state.lastRun.review.reasons ?? ['Inspect REVIEW.md.']).join('; '), {
        env, apiKeyEnv: state.config.llm.api_key_env,
      })}\n` +
        (isReviewRequired(state.config)
          ? 'Publication is blocked unless explicitly bypassed. Correct the evidence and retry.\n'
          : 'WARNING: the configured review gate is disabled; publication would bypass this failed review, not approve it. Correct the evidence and retry.\n')
      : 'Use /publish to publish reviewed changes with --merge-when-green.\n');
    if (state.askQueue.length && !state.pendingConfirm && !state.lastRun.planMode && !state.lastRun.planningOnly && !state.lastRun.failed) {
      const next = state.askQueue.shift();
      output.write(`Starting queued Ask: ${next}\n`);
      return runRequest({ kind: 'ask', text: next, options: {} });
    }
    return true;
  }

  function runRequest(request, { retry = false, continueConfirmed = false } = {}) {
    if (retry && !request.prepared?.worktreePath) {
      throw new Error('No prepared worktree is available to retry; start the Ask or issue run first.');
    }
    const options = { ...request.options,
      ...(state.routeNext ? { autoModel: true } : {}),
      ...(request.kind === 'run' ? { issue: Number(request.issue) } : {}),
      ...(retry ? { preparedRun: request.prepared } : {}),
      ...(continueConfirmed ? { confirm: false,
        ...(request.options.planMode ? { planMode: false, acceptPlan: true } : {}) } : {}),
    };
    if (continueConfirmed && request.options.planMode) request.options = { ...request.options, planMode: false };
    const run = request.kind === 'ask' ? (options) => api.runBuiltinAsk(request.text, options)
      : (options) => api.runBuiltinIssue(request.issue, options);
    return executeRun(run, options, request);
  }

  async function dispatch(line) {
    if (typeof line !== 'string') throw new TypeError('Shell input must be text');
    if (state.pendingSecret !== null) {
      const name = state.pendingSecret;
      state.pendingSecret = null;
      await api.createFileVault().set(name, line);
      output.write(`Stored secret ${name}.\n`);
      return true;
    }
    const text = line.trim();
    if (!text) {
      if (state.pendingConfirm) return runRequest(state.pendingConfirm, { retry: true, continueConfirmed: true });
      if (state.askQueue.length && state.controller === null) {
        const next = state.askQueue.shift();
        output.write(`Starting queued Ask: ${next}\n`);
        return runRequest({ kind: 'ask', text: next, options: {} });
      }
      return true;
    }
    if (text === 'exit') { cancel(); return false; }
    if (state.controller !== null && !text.startsWith('/')) {
      if (state.display.seat === 'coder' && state.steeringControl?.waiting) {
        state.steeringControl.steer(text);
        output.write('Steering the coder.\n');
        return true;
      }
      queueAsk(text);
      return true;
    }
    if (text === '/') { output.write(formatHelp()); return true; }
    if (!text.startsWith('/')) return runRequest({ kind: 'ask', text, options: {} });
    const match = /^\/([a-z]+)(?:\s+(.*))?$/.exec(text);
    if (!match) {
      errorOutput.write(unknownCommand);
      return true;
    }
    const [, inputCommand, rawArguments] = match;
    const command = canonicalCommand(inputCommand);
    const args = rawArguments?.trim() ?? '';
    switch (command) {
      case 'steer': {
        if (!args) throw new TypeError('Use /steer TEXT.');
        if (state.display.seat !== 'coder' || !state.steeringControl?.waiting) {
          throw new Error('Steering requires a drafting coder model call; it cannot change planner or reviewer scope.');
        }
        const instruction = [...state.queuedInput, args].join('\n');
        if (instruction.length > 4096 || redactEvidence(instruction, { env, apiKeyEnv: state.config.llm.api_key_env }) !== instruction) {
          throw new TypeError('Steering instruction exceeds 4096 characters or contains secret material.');
        }
        state.steeringControl.steer(instruction);
        state.queuedInput = [];
        output.write('Steering the coder.\n');
        return true;
      }
      case 'map':
        if (args) throw new TypeError('Use /map.');
        if (!state.lastRun?.worktreePath) throw new Error('Run or resume a task before /map.');
        {
          const map = await api.writeRepoMap({ worktree: state.lastRun.worktreePath, env, apiKeyEnv: state.config.llm.api_key_env });
          output.write(`Repo map: ${map.lines} filename-only lines (cap 80).\n`);
        }
        return true;
      case 'waves': {
        if (args && args !== 'open') throw new TypeError('Use /waves or /waves open.');
        if (!state.lastRun?.worktreePath) throw new Error('Run or resume a plan worktree before /waves.');
        const rows = await api.waveBoard({ worktree: state.lastRun.worktreePath, cwd: currentRoot(), env,
          apiKeyEnv: state.config.llm.api_key_env, open: args === 'open',
          activeIssue: state.display.issue, activeState: state.display.state });
        safeWrite(rows.map((row) => `wave:${row.wave} | ${row.title} | ${row.state}` +
          (row.issue ? ` | #${row.issue}` : '')).join('\n') + '\n');
        return true;
      }
      case 'review': {
        if (args && args !== '--again') throw new TypeError('Use /review [--again].');
        if (state.controller !== null) throw new Error('Stop the active seat before starting an explicit review.');
        const controller = new AbortController();
        state.controller = controller;
        state.display.busy = true;
        state.display.seat = 'reviewer';
        state.display.state = 'reviewing';
        notify();
        try {
          state.lastRun = await api.runOnlyReview(state.lastRun, { repoRoot, config: state.config, env,
            again: args === '--again', signal: controller.signal, onRunEvent: receiveEvent,
            debug: state.debug, errorOutput });
          state.display.review = state.lastRun.review.verdict;
          state.display.state = state.lastRun.review.verdict === 'pass' ? 'passed' : 'failed';
          output.write(`Review: ${state.lastRun.review.verdict}. ${state.lastRun.review.verdict === 'fail'
            ? 'Publication is blocked unless /publish --skip-review is explicit.' : 'Read-only review complete.'}\n`);
        } catch (error) {
          state.display.state = isRunCancelled(error) ? 'idle' : 'failed';
          state.display.review = null;
          if (state.lastRun) state.lastRun = { ...state.lastRun, review: null };
          throw error;
        } finally {
          discardQueued(controller.signal);
          state.controller = null;
          state.display.busy = false;
          notify();
        }
        return true;
      }
      case 'worktrees': {
        if (args) throw new TypeError('Use /worktrees.');
        const worktrees = await api.listIssueWorktrees({ cwd: currentRoot(), env });
        output.write(worktrees.length ? worktrees.map((entry) =>
          redactEvidence(`${entry.path} | ${entry.branch} | ${entry.seat} | ${entry.status}`, {
            env, apiKeyEnv: state.config.llm.api_key_env,
          }).replace(/[\x00-\x1f\x7f]/g, '?')).join('\n') + '\n' : 'No registered issue worktrees.\n');
        return true;
      }
      case 'batch':
        throw new Error('One seat at a time. Worktrees are isolated.');
      case 'context':
        if (args) throw new TypeError('Use /context.');
        safeWrite(formatContext(state.lastMeasuredSeat, { packBudgetChars: state.config.seat.context_chars, env }));
        return true;
      case 'usage':
        if (args) throw new TypeError('Use /usage.');
        safeWrite(formatUsage(state.display, state.lastMeasuredSeat));
        return true;
      case 'btw': {
        if (!args) throw new TypeError('Use /btw QUESTION.');
        if (state.sideController) throw new Error('A read-only side question is already active.');
        const controller = new AbortController();
        state.sideController = controller;
        try {
          const answer = await api.askSideQuestion({ question: args, run: state.lastRun,
            config: state.config, env, signal: controller.signal });
          safeWrite(`${answer}\n`);
        } finally {
          state.sideController = null;
        }
        return true;
      }
      case 'resume': {
        if (!args) {
          const runs = await api.listLocalRuns({ cwd: currentRoot(), config: state.config, env });
          if (!runs.length) output.write('No local issue runs.\n');
          for (const run of runs) safeWrite(
            `#${run.issue.number} | ${run.issue.title.replace(/[\x00-\x1f\x7f]/g, '?')} | ${run.seat} | ${run.state} | ${run.task}\n`);
          return true;
        }
        if (!/^[1-9]\d*$/.test(args) || !Number.isSafeInteger(Number(args))) throw new TypeError('Use /resume [N].');
        const prepared = await api.readLocalRun({ number: Number(args), cwd: currentRoot(), config: state.config, env });
        if (prepared.planMode) {
          const request = { kind: 'run', issue: args, options: { planMode: true }, prepared };
          return executeRun(async () => prepared, { issue: Number(args), planMode: true }, request);
        }
        return runRequest({ kind: 'run', issue: args, options: {}, prepared }, { retry: true });
      }
      case 'recap':
        if (args) throw new TypeError('Use /recap.');
        safeWrite(await api.recapRun(state.lastRun, { finishReason: state.display.lastFinishReason, env }));
        return true;
      case 'checkpoints':
      case 'rewind': {
        const run = state.lastRun;
        if (!run?.worktreePath || !run.task) throw new Error('Run or resume a task worktree before using checkpoints.');
        if (command === 'checkpoints') {
          if (args) throw new TypeError('Use /checkpoints.');
          const records = await api.listCheckpoints({ worktree: run.worktreePath, task: run.task });
          output.write(records.length ? records.map((record) =>
            `${record.number} | ${record.seat} | ${record.status} | ${record.time}`).join('\n') + '\n' : 'No checkpoints.\n');
          return true;
        }
        const latest = inputCommand === 'undo';
        if (latest ? Boolean(args) : !/^[1-9]\d*$/.test(args) || !Number.isSafeInteger(Number(args))) {
          throw new TypeError(latest ? 'Use /undo for the latest checkpoint only.' : 'Use /rewind N.');
        }
        if (state.controller !== null) throw new Error('Stop the active seat before rewinding product files.');
        const task = await api.readPlannerTask(run.worktreePath);
        if (task === null) throw new Error('Rewind requires the current TASK.md product scope.');
        const checkpoint = await api.rewindCheckpoint({ worktree: run.worktreePath, task: run.task,
          number: latest ? undefined : Number(args), published: state.published, env,
          allowedFiles: taskAndRepairFiles(taskFilesAllowed(task), run.result?.repairFiles, run.result?.scopeFiles) });
        state.lastRun = { ...run, review: null, ...(run.result ? { result: { ...run.result,
          excellence: { ...run.result.excellence, pass: false, reasons: ['Product files were rewound; rerun verification.'] } } } : {}) };
        state.display.state = 'idle';
        state.display.review = null;
        notify();
        output.write(`Rewound product files to checkpoint ${checkpoint.number}. TASK, plan, and logs were kept; rerun checks before publishing.\n`);
        return true;
      }
      case 'plan':
        if (!args) throw new TypeError('Use /plan TEXT, or /run N --plan.');
        return runRequest({ kind: 'ask', text: args, options: { planMode: true } });
      case 'issues': {
        if (args) throw new TypeError('Use /issues.');
        const issues = await api.listOpenIssues({ cwd: currentRoot(), env });
        if (!issues.length) output.write('No open issues.\n');
        for (const issue of issues) {
          const cached = state.issueCache.get(issue.number);
          state.issueCache.set(issue.number, { ...cached, issue: { ...cached?.issue, ...issue },
            branch: cached?.branch ?? `issue-${issue.number}` });
          safeWrite(`#${issue.number} ${issue.title.replace(/[\x00-\x1f\x7f]/g, '?')}\n`);
        }
        if (issues.length === 100) output.write('Showing the first 100 open issues; use GitHub for the remaining board.\n');
        return true;
      }
      case 'issue': {
        if (!/^[1-9]\d*$/.test(args) || !Number.isSafeInteger(Number(args))) throw new TypeError('Use /issue N.');
        const number = Number(args);
        let cached = state.issueCache.get(number);
        if (!cached) {
          cached = await api.readStatus({ issue: number, cwd: currentRoot(), config: state.config, env });
          state.issueCache.set(number, cached);
        }
        safeWrite(api.formatIssueSummary(cached, { env }));
        return true;
      }
      case 'diff': {
        if (args) throw new TypeError('Use /diff.');
        const worktree = state.lastRun?.worktreePath ??
          (/^issue-[1-9]\d*$/.test(state.display.branch) ? currentRoot() : null);
        if (!worktree) throw new Error('No current issue worktree; run an Ask or /run N before /diff.');
        const names = await api.readDiffNames({ cwd: worktree, env });
        safeWrite(names.length ? `${names.map((name) => name.replace(/[\x00-\x1f\x7f]/g, '?')).join('\n')}\n` : 'No tracked diff.\n');
        return true;
      }
      case 'stop':
        if (args) throw new TypeError('Use /stop.');
        if (!cancel()) output.write('No active run.\n');
        return true;
      case 'retry':
        if (args) throw new TypeError('Use /retry.');
        if (!state.lastRequest) throw new Error('No Ask or issue run is available to retry.');
        if (state.lastRequest.options.planMode) throw new Error('Press Enter to accept the plan, or /stop to leave it unchanged.');
        return runRequest(state.lastRequest, { retry: true, continueConfirmed: true });
      case 'redraw':
      case 'clear':
        if (args) throw new TypeError(`Use /${command}.`);
        onUiAction(command);
        notify();
        return true;
      case 'statusbar':
        if (!['on', 'off'].includes(args)) throw new TypeError('Use /statusbar on or /statusbar off.');
        state.statusbar = args === 'on';
        notify();
        output.write(`Status bars ${args}.\n`);
        return true;
      case 'debug': {
        if (!['on', 'off', 'status'].includes(args)) throw new TypeError('Use /debug on, /debug off, or /debug status.');
        if (args !== 'status') state.debug.setEnabled(args === 'on');
        notify();
        output.write(`Debug logging ${state.debug.enabled ? 'on' : 'off'}.\n`);
        return true;
      }
      case 'history': {
        if (args) throw new TypeError('Use /history.');
        const lines = state.history.filter((line) => safeHistoryLine(line, { env })).slice(-20);
        output.write(lines.length ? `${lines.join('\n')}\n` : 'No stored commands.\n');
        return true;
      }
      case 'queue': {
        if (!args || args === 'list') { listAskQueue(); return true; }
        if (args === 'clear') {
          state.askQueue = [];
          output.write('Ask queue cleared.\n');
          notify();
          return true;
        }
        const drop = /^drop\s+([1-8])$/.exec(args);
        if (drop) {
          const index = Number(drop[1]) - 1;
          if (!state.askQueue[index]) throw new TypeError('No queued Ask at that number. Use /queue list.');
          const [removed] = state.askQueue.splice(index, 1);
          output.write(`Dropped ${index + 1}. ${removed}\n`);
          notify();
          return true;
        }
        queueAsk(args);
        return true;
      }
      case 'ask': {
        if (!args) throw new TypeError('Use /ask TEXT.');
        return runRequest({ kind: 'ask', text: args, options: {} });
      }
      case 'model': {
        if (!args) {
          safeWrite(`Model: ${state.config.llm.model || '(unset)'}\nHost: ${host()}\n`);
          return true;
        }
        const selection = /^([^\s-]\S*)(?:\s+(--save))?$/.exec(args);
        if (!selection) throw new TypeError('Use /model [ID|clear] [--save].');
        await selectModel(selection[1], selection[2] === '--save');
        return true;
      }
      case 'effort': {
        if (!args || args === 'status') {
          output.write(`Effort: ${state.config.llm.effort}\n`);
          return true;
        }
        if (!['l', 'm', 'h', 'x', 'none'].includes(args)) throw new TypeError('Use /effort l|m|h|x|none|status.');
        state.config = { ...state.config, llm: { ...state.config.llm, effort: args, effort_override: args } };
        syncModelDisplay();
        output.write(`Effort: ${state.config.llm.effort}\n`);
        return true;
      }
      case 'provider':
        if (args) throw new TypeError('Use /provider.');
        safeWrite(`Profile: ${state.fleetProfileId ?? (state.config.llm.profile || '(custom)')}\nHost: ${host()}\n`);
        return true;
      case 'fleet': {
        const words = splitArguments(args);
        const command = words[0] ?? 'list';
        if (command === 'add') {
          await api.runFleet(words, { cwd: currentRoot(), installationRoot: repoRoot, env,
            input, output: { isTTY: output.isTTY, write: safeWrite }, errorOutput,
            ...(askInput ? { question: askInput } : {}) });
          return true;
        }
        if (!['list', 'use', 'probe'].includes(command)) throw new TypeError('Use /fleet [list|use ID|probe [ID] [--set-model [MODEL]]|add FLAGS].');
        const fleet = await api.loadFleet({ cwd: currentRoot() });
        if (command === 'list') {
          if (words.length > 1) throw new TypeError('Use /fleet or /fleet list.');
          if (!fleet.profiles.length) output.write('No fleet profiles configured.\n');
          for (const profile of fleet.profiles) safeWrite(
            `${profile.id} | ${new URL(profile.base_url).host} | ${profile.model} | ctx ${profile.context_max || '-'}\n`);
          return true;
        }
        if (command === 'use') {
          if (words.length !== 2) throw new TypeError('Use /fleet use ID.');
          const profile = api.getFleetProfile(fleet, words[1]);
          state.config = api.withFleetProfile(state.config, profile);
          state.fleetProfileId = profile.id;
          state.routeNext = false;
          syncModelDisplay();
          safeWrite(`Session profile: ${profile.id}\nModel: ${profile.model}\nHost: ${host()}\n`);
          return true;
        }
        const rest = words.slice(1);
        const id = rest[0] && !rest[0].startsWith('--') ? rest.shift() : null;
        const save = rest[0] === '--set-model';
        const supplied = save ? rest[1] : undefined;
        if (rest.length && (!save || rest.length > 2 || supplied?.startsWith('--'))) {
          throw new TypeError('Use /fleet probe [ID] [--set-model [MODEL]].');
        }
        const active = id ? api.withFleetProfile(state.config, api.getFleetProfile(fleet, id)) : state.config;
        if (!active.llm.base_url) throw new Error('Select a session endpoint with /fleet use ID before probing.');
        const models = await api.probeModelDetails(active.llm.base_url, { env, apiKeyEnv: active.llm.api_key_env });
        safeWrite(`Models at ${host(active)}:\n` + models.map(({ id, context_max: capacity }) =>
          `  ${id} | ctx ${capacity ?? '-'}\n`).join(''));
        if (save) {
          const model = supplied ?? (models.some(({ id }) => id === active.llm.model)
            ? active.llm.model : models.length === 1 ? models[0].id : null);
          if (!model || !models.some(({ id }) => id === model)) {
            throw new TypeError('Choose a listed model with /fleet probe --set-model MODEL.');
          }
          await selectModel(model, true);
        }
        return true;
      }
      case 'run': {
        const issue = /^(?:--issue\s+)?([1-9]\d*)((?:\s+--[a-z-]+(?:\s+[1-9]\d*)?)*)$/.exec(args);
        const flags = issue?.[2].trim().split(/\s+/).filter(Boolean) ?? [];
        let parallel;
        let attempts;
        const attemptPosition = flags.indexOf('--attempts');
        if (attemptPosition !== -1 && /^[1-9]\d*$/.test(flags[attemptPosition + 1] ?? '')) {
          attempts = Number(flags[attemptPosition + 1]);
          flags.splice(attemptPosition, 2);
        }
        const position = flags.indexOf('--parallel');
        if (position !== -1 && /^[1-9]\d*$/.test(flags[position + 1] ?? '')) {
          parallel = Number(flags[position + 1]);
          flags.splice(position, 2);
        }
        if (!issue || flags.some((flag) => !['--auto-model', '--saved', '--confirm', '--plan'].includes(flag)) ||
            parallel !== undefined && (!Number.isSafeInteger(parallel) ||
              parallel > 1 && (flags.includes('--confirm') || flags.includes('--plan'))) ||
            attempts !== undefined && (!Number.isSafeInteger(attempts) ||
              attempts > 1 && (parallel > 1 || flags.includes('--confirm') || flags.includes('--plan'))) ||
            new Set(flags).size !== flags.length) {
          throw new TypeError('Use /run N [--parallel K] [--attempts K] [--saved] [--confirm|--plan].');
        }
        return runRequest({ kind: 'run', issue: issue[1], options: {
          autoModel: !flags.includes('--saved'), confirm: flags.includes('--confirm'),
          planMode: flags.includes('--plan'),
          ...(parallel === undefined ? {} : { parallel }),
          ...(attempts === undefined ? {} : { attempts }),
        } });
      }
      case 'status': {
        const fields = args ? args.split(/\s+/) : [];
        const offline = fields.includes('--offline');
        const numbers = fields.filter((field) => field !== '--offline');
        if (fields.filter((field) => field === '--offline').length > 1 ||
            numbers.length > 1 || (numbers.length && !/^[1-9]\d*$/.test(numbers[0]))) {
          throw new TypeError('Use /status [N] [--offline].');
        }
        const issue = numbers[0] ?? state.lastRun?.issue?.number ?? state.lastAsk?.number;
        if (numbers.length === 0 || Number(issue) === state.display.issue) {
          output.write(formatSessionStatus(state, { env }));
          return true;
        }
        const cached = state.issueCache.get(Number(issue));
        if (cached?.display) {
          output.write(formatSessionStatus({ display: cached.display, lastRun: cached }, { env }));
          return true;
        }
        const status = await api.readStatus({
          issue, offline, cwd: currentRoot(), config: state.config,
        });
        state.issueCache.set(status.issue.number, status);
        output.write(api.formatStatus(status));
        return true;
      }
      case 'remediate': {
        const files = args.split(/\s+/).filter(Boolean);
        const failing = files.length ? files : state.lastRun?.baselineFailures ?? [];
        const task = remediationTask(failing, currentRoot());
        const profile = selectRemediationProfile(state.fleet?.profiles, state.fleetProfileId);
        if (profile) {
          state.fleetProfileId = profile.id;
          state.config = api.withFleetProfile(state.config, profile);
          syncModelDisplay();
        }
        safeWrite(`Remediation agent for ${currentRoot()}\nFiles: ${task.files.join(', ')}\n` +
          `Profile: ${profile?.id ?? state.fleetProfileId ?? 'current'}\n`);
        return runRequest({ kind: 'ask', text: task.ask, options: {} });
      }
      case 'eval': {
        if (env.ROSTER_SEAT) throw new Error('AI-Eval is human-only; an agent seat cannot record an evaluation.');
        const { values, options } = parseShellEvaluationArgs(args);
        const evaluation = await api.recordEvaluation(...values, { ...options, cwd: currentRoot(), env });
        output.write(`Recorded AI-Eval for ${evaluation.sha ?? evaluation.session} verdict ${evaluation.verdict} difficulty ${evaluation.difficulty} minutes ${evaluation.minutes ?? '-'} path ${evaluation.path}\n`);
        return true;
      }
      case 'log': {
        if (args === 'debug') {
          if (!state.debug.enabled) throw new Error('Debug logging is off. Use /debug on before /log debug.');
          const log = await state.debug.tail({ limit: 50 });
          output.write(log?.lines.length ? `${log.lines.join('\n')}\n` : 'No debug events recorded in this process.\n');
          return true;
        }
        if (!/^[1-9]\d*$/.test(args) || !Number.isSafeInteger(Number(args))) throw new TypeError('Use /log N.');
        const logs = await api.readIssueLogs({ repoRoot: currentRoot(), issue: Number(args), env,
          apiKeyEnv: state.config.llm.api_key_env, limit: 50 });
        if (!logs.length) output.write(`No local run logs for issue #${args}.\n`);
        for (const log of logs) output.write(`${log.path}\n${log.lines.join('\n')}\n`);
        return true;
      }
      case 'publish': {
        if (state.lastRun?.parallelRun) throw new Error('Parallel runs have separate child worktrees; publish each child through its reviewed App SDK handoff.');
        if (state.lastRun?.attempt && state.lastRun.attempt.winner !== state.lastRun.attempt.index) {
          throw new Error('Only the recorded winning attempt may be published; review bypass cannot select a loser');
        }
        const attemptEvidence = state.lastRun?.attemptResults
          ? (await import('./lib/attempts.mjs')).attemptSummary(state.lastRun) : '';
        requirePublicationEnabled(state.config);
        if (state.lastRun?.askKind && state.lastRun.askKind !== 'slice') {
          throw new Error('Planning-only PLAN or clarification is not code to publish; create and run a bounded slice first.');
        }
        if (state.lastRun?.planningOnly) throw new Error('Planning-only TASK is not code to publish; /run the validated task first.');
        if (state.lastRun && state.published) {
          throw new Error('This run was already published; start another /run before publishing again.');
        }
        const { skipReview, subject: requestedSubject, model: requestedModel } = parsePublishArgs(args);
        const reviewBypass = skipReview || !isReviewRequired(state.config);
        const reviewLabel = skipReview ? 'gate bypassed with --skip-review'
          : !isReviewRequired(state.config) ? 'gate not required by configuration' : 'pass';
        const subject = requestedSubject || (state.lastRun
          ? state.lastRun.local ? 'feat: local ask' : `feat: issue ${state.lastRun.issue.number}` : null);
        if (subject !== null) conventionalSubject(subject);
        const appId = Boolean(env.GITHUB_APP_ID);
        const keyPath = Boolean(env.GITHUB_APP_PRIVATE_KEY_PATH);
        if (appId !== keyPath) {
          throw new Error('Set both GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH to publish.');
        }
        await requirePassingReview(state.lastRun, reviewBypass);
        if (!appId) {
          const ghcp = !state.lastRun?.runs?.coder;
          if (ghcp) resolvePublishModel({ env, model: requestedModel, ghcp: true });
          const publishEnv = buildPublishEnv({
            config: state.config, env, run: state.lastRun?.runs?.coder,
            model: requestedModel, task: ghcp
              ? api.publicationTask({ cwd: currentRoot(), env, task: state.lastRun?.task }) : undefined,
          });
          const message = buildPublishMessage({
            subject: subject ?? '<conventional subject>', model: publishEnv.AI_MODEL,
            summary: redactEvidence(state.lastRun?.result?.summary ?? subject ?? '<describe the reviewed changes>', {
              env, apiKeyEnv: state.config.llm.api_key_env,
            }),
            testsSkipped: state.lastRun?.result?.testsSkipped,
            scopeFiles: state.lastRun?.result?.scopeFiles ?? [],
            issueNumber: state.lastRun?.issue?.number,
            seats: state.lastRun
              ? `planner, coder, reviewer (${reviewLabel})`
              : undefined,
            ghcp,
          }) + attemptEvidence;
          api.resolveContractsPath({ repoRoot, cwd, env });
          output.write(`From ${state.lastRun?.worktreePath ?? currentRoot()}, publish reviewed changes:\n` +
            `Set these metadata values (empty values clear inherited fields):\n${formatPublishEnvironment(publishEnv)}` +
            `${formatPublishCommand({ message, model: publishEnv.AI_MODEL })}\n`);
          return true;
        }
        if (subject === null) throw new TypeError('Use /publish <conventional subject> or /run N first.');
        let contractsPath;
        let publishEnv;
        let publishRoot;
        if (state.lastRun) {
          ({ contractsPath, publishEnv, worktreePath: publishRoot } =
            await api.prepareBuiltinPublication(state.lastRun, {
              cwd, config: state.config, env, skipReview,
            }));
        } else {
          resolvePublishModel({ env, model: requestedModel, ghcp: true });
          publishRoot = currentRoot();
          contractsPath = api.resolveContractsPath({ repoRoot: publishRoot, cwd, env });
          publishEnv = buildPublishEnv({ config: state.config, env, model: requestedModel,
            task: api.publicationTask({ cwd: publishRoot, env }) });
          publishEnv.GITHUB_APP_PRIVATE_KEY_PATH = resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH);
        }
        const model = publishEnv.AI_MODEL;
        const message = buildPublishMessage({
          subject, model, issueNumber: state.lastRun?.issue?.number,
          summary: redactEvidence(state.lastRun ? state.lastRun.result.summary
            : subject.replace(/^[a-z]+(?:\([^)]+\))?!?: /, ''), {
            env, apiKeyEnv: state.config.llm.api_key_env,
          }),
          testsSkipped: state.lastRun?.result?.testsSkipped,
          scopeFiles: state.lastRun?.result?.scopeFiles ?? [],
          seats: state.lastRun
            ? `planner, coder, reviewer (${reviewLabel})`
            : undefined,
          ghcp: !state.lastRun,
        }) + attemptEvidence;
        const commentOnIssue = async (pullNumber) => {
          state.published = true;
          if (state.lastRun.local) {
            output.write(`Human AI-Eval after merge (replace M with actual minutes):\n` +
              `${humanEvalHint(state.lastRun.sessions.coder)}\n`);
            return;
          }
          await api.issueCommenter({
            issue: state.lastRun.issue, pullNumber, model,
            runLine: state.lastRun.runs?.coder?.line, run: state.lastRun.runs?.coder,
            repoRoot: state.lastRun.repoRoot, cwd, env,
          });
          output.write(`Commented on issue #${state.lastRun.issue.number}; left it open for human AI-Eval.\n`);
          output.write(`Human AI-Eval after merge (replace M with actual minutes):\n` +
            `${humanEvalHint(state.lastRun.runs?.coder?.metrics?.session ??
              state.lastRun.runs?.coder?.env?.AI_SESSION ?? `roster-${state.lastRun.issue.number}-coder`)}\n`);
        };
        let publication;
        try {
          publication = await api.publisher({
            contractsPath, cwd: publishRoot, env: publishEnv, message, model,
            output, errorOutput,
          });
        } catch (error) {
          if (state.lastRun && Number.isSafeInteger(error.mergedPullRequest) &&
              error.mergedPullRequest > 0) {
            await commentOnIssue(error.mergedPullRequest);
          }
          throw error;
        }
        if (state.lastRun) {
          if (!Number.isSafeInteger(publication?.mergedPullRequest) || publication.mergedPullRequest <= 0) {
            throw new Error('Publisher did not confirm a merged PR; issue remains open');
          }
          await commentOnIssue(publication.mergedPullRequest);
        } else state.published = true;
        return true;
      }
      case 'stats': {
        if (args && /\s/.test(args)) throw new TypeError('Use /stats [REF].');
        output.write(api.formatMetrics(api.summarizeMetrics(metrics(args))));
        output.write(api.formatFailureProposals(await api.listFailureProposals({ cwd: currentRoot() })));
        return true;
      }
      case 'learn': {
        if (args !== '--recurring') throw new TypeError('Use /learn --recurring.');
        const result = await api.proposeRecurringFailures({ cwd: currentRoot(), env,
          apiKeyEnv: state.config.llm.api_key_env });
        output.write(`Recurring failures: ${result.created.length} draft proposals created; ` +
          `${result.existing.length} existing drafts preserved. Human review required.\n`);
        return true;
      }
      case 'recommend': {
        const metadata = state.lastRun?.planner?.metadata ?? state.lastRun?.result?.taskMetadata;
        const { taskClass, difficulty } = args
          ? parseRecommendationArgs(['--task-class', ...args.split(/\s+/)])
          : { taskClass: metadata?.task_class ?? inferTaskClass(state.lastRun?.issue?.title ?? '') ?? 'feat',
            difficulty: metadata?.difficulty ?? 2 };
        const root = state.lastRun?.repoRoot ?? resolveProjectRoot(cwd);
        const route = await api.routeTask({ cwd: root, installationRoot: repoRoot, taskClass,
          difficulty: difficulty ?? 2, records: api.loadAvailableMetrics({ cwd: root }) });
        output.write(api.formatRoute(route, taskClass, state.config, env));
        return true;
      }
      case 'doctor': {
        if (!args) {
          const result = await api.checkDoctor({ cwd: currentRoot(), installationRoot: repoRoot, env });
          safeWrite(api.formatDoctor(result));
          return true;
        }
        if (args !== 'warm') throw new TypeError('Use /doctor or /doctor warm.');
        if (state.controller !== null) throw new Error('A seat is already running; cancel it before warming.');
        const controller = new AbortController();
        state.controller = controller;
        state.display.busy = true;
        notify();
        try {
          const result = await api.warmDoctor({ cwd: currentRoot(), installationRoot: repoRoot, env,
            config: state.config, signal: controller.signal, errorOutput: { write() {} } });
          if (result?.skipped !== true && !Number.isInteger(result?.status)) throw new Error('Warm probe returned no status.');
          safeWrite(`Warm probe: host=${host()} status=${result.skipped ? 'skipped' : result.status}\n`);
        } finally {
          discardQueued(controller.signal);
          state.controller = null;
          state.display.busy = false;
          notify();
        }
        return true;
      }
      case 'config': {
        const words = splitArguments(args);
        if (!words.length) {
          safeWrite(api.publicConfig({ repoRoot, cwd, env }));
          return true;
        }
        if (words[0] === 'path' && words.length === 1) {
          output.write(`${api.privateConfigPath({ repoRoot, cwd })}\n`);
          return true;
        }
        if (words[0] !== 'set' || words.length !== 3) throw new TypeError('Use /config, /config path, or /config set KEY VALUE.');
        const setting = configSetting(words[1], words[2], { env, apiKeyEnv: state.config.llm.api_key_env });
        if (setting.field === 'statusbar') state.statusbar = setting.value;
        else if (setting.field === 'debug') state.debug.setEnabled(setting.value);
        else {
          const saved = await api.setConfigValue(setting.field, setting.value, { repoRoot, cwd });
          state.config = setting.field === 'effort'
            ? { ...state.config, llm: { ...state.config.llm, effort: saved.llm.effort, effort_override: saved.llm.effort_override } }
            : { ...state.config, seat: { ...state.config.seat, context_chars: saved.seat.context_chars },
              ...(state.config.context ? { context: { ...state.config.context, budget: saved.seat.context_chars } } : {}) };
          syncModelDisplay();
        }
        notify();
        output.write(`Config ${setting.field} set${['statusbar', 'debug'].includes(setting.field) ? ' for this process' : ''}.\n`);
        return true;
      }
      case 'vault': {
        if (!args || args === 'list') {
          const names = await api.createFileVault().list();
          output.write(names.length ? `${names.join('\n')}\n` : 'No secrets stored.\n');
          return true;
        }
        const lookup = /^get\s+(\S+)$/.exec(args);
        if (lookup) {
          api.validateSecretName(lookup[1]);
          const value = await api.createFileVault().get(lookup[1]);
          if (value === undefined) throw new Error(`No secret stored for ${lookup[1]}.`);
          output.write(`${lookup[1]} is stored (value hidden; pipe roster vault get NAME to retrieve it).\n`);
          return true;
        }
        const secret = /^set\s+(\S+)$/.exec(args);
        if (!secret) throw new TypeError('Use /vault [list], /vault get NAME, or /vault set NAME.');
        api.validateSecretName(secret[1]);
        state.pendingSecret = secret[1];
        output.write('Secret (input hidden): ');
        return true;
      }
      case 'help':
        if (/\s/.test(args)) throw new TypeError('Use /help [GROUP|COMMAND].');
        {
          const help = formatHelp(args);
          if (help === null) errorOutput.write(unknownCommand);
          else output.write(help);
        }
        return true;
      case 'quit':
        if (args) throw new TypeError('Use /quit.');
        cancel();
        return false;
      default:
        errorOutput.write(unknownCommand);
        return true;
    }
  }

  return {
    dispatch,
    state,
    banner: 'github-agent-roster',
    cancel,
    queueInput,
  };
}

export async function startRepl({
  input = process.stdin,
  output = process.stdout,
  errorOutput = process.stderr,
  historyRoot,
  historyStore,
  ...options
} = {}) {
  let tray;
  let transcript;
  const terminal = input.isTTY === true;
  const messages = { isTTY: output.isTTY, write(text) { transcript?.reset(); if (tray) tray.write(text); else output.write(text); } };
  const errors = { write(text) { transcript?.reset(); if (tray) tray.write(text, errorOutput); else errorOutput.write(text); } };
  let question;
  let shell;
  const { dispatch, state, banner, cancel, queueInput } = createDispatcher({ ...options, input, output: messages, errorOutput: errors,
    onStateChange() { if (tray) tray.render(); },
    onUiAction(command) { if (command === 'clear') { tray?.erase(); output.write('\x1b[2J\x1b[H'); } },
    askInput(prompt) {
      if (question) throw new Error('A shell question is already active.');
      tray?.erase();
      state.pendingQuestion = true;
      shell.setPrompt(prompt);
      shell.prompt();
      return new Promise((resolve, reject) => { question = { resolve, reject }; });
    },
  });
  const history = historyStore ?? createHistory({
    repoRoot: historyRoot ?? resolveProjectRoot(options.cwd ?? process.cwd()), env: options.env ?? process.env,
  });
  state.history = await history.load();
  let historyError;
  let suppressEcho = false;
  const handledLines = [];
  const sideRequests = new Set();
  let exitCode = 0;
  const terminalOutput = new Writable({
    write(chunk, encoding, callback) {
      if (!suppressEcho && state.pendingSecret === null) output.write(chunk, encoding);
      callback();
    },
  });
  terminalOutput.isTTY = terminal;
  terminalOutput.columns = output.columns ?? 80;
  shell = createInterface({ input, output: terminalOutput, terminal, historySize: 200,
    history: [...state.history].reverse(), removeHistoryDuplicates: true, completer: completeCommand });
  if (terminal) {
    tray = createTray({ output, state, shell, env: options.env ?? process.env,
      cwd: options.cwd ?? process.cwd(), services: options.services ?? {} });
    transcript = createTranscript({
      write: (text, writeOptions) => tray.write(text, output, writeOptions),
      columns: () => output.columns ?? 80,
    });
    state.transcript = transcript;
  }
  shell.on('SIGINT', () => {
    if (cancel()) return;
    exitCode = 130;
    shell.close();
  });
  shell.on('close', () => { cancel(); question?.reject(new RunCancelledError()); question = null; });
  shell.on('history', (entries) => {
    const secret = suppressEcho || state.pendingSecret !== null || state.pendingQuestion;
    const safe = entries.filter((line, index) => !(secret && index === 0) &&
      safeHistoryLine(line, { env: options.env ?? process.env }));
    entries.splice(0, entries.length, ...safe);
  });
  shell.on('line', (line) => {
    tray?.committed();
    const answer = question;
    const sideQuestion = /^\/btw\s+\S/.test(line.trim()) && state.controller !== null && !answer;
    const steering = /^\/steer(?:\s|$)/.test(line.trim()) && state.controller !== null && !answer;
    const queued = state.controller !== null && !answer &&
      !suppressEcho && state.pendingSecret === null && line.trim() && !line.trim().startsWith('/') && line.trim() !== 'exit';
    const activeStop = line.trim() === '/stop' && state.controller !== null;
    handledLines.push(activeStop || sideQuestion || steering || Boolean(queued) || Boolean(answer));
    if (queued) {
      try { queueInput(line); }
      catch (error) { errors.write(`${error.message}\n`); }
    }
    if (sideQuestion || steering) {
      const pending = dispatch(line).catch((error) => errors.write(`${error.message}\n`))
        .finally(() => sideRequests.delete(pending));
      sideRequests.add(pending);
    }
    if (answer) { question = null; state.pendingQuestion = false; answer.resolve(line); }
    if (activeStop) cancel();
    if (state.controller !== null && ['/quit', '/q', 'exit'].includes(line.trim())) {
      cancel();
      shell.close();
    }
    const secret = suppressEcho || state.pendingSecret !== null || Boolean(answer);
    history.record(line, { secret }).then(() => { state.history = history.lines; }).catch((error) => {
      historyError ??= error;
      errors.write(`${error.message}\n`);
    });
    if (suppressEcho) suppressEcho = false;
    else if (/^\/vault set [A-Za-z_][A-Za-z0-9_]{0,63}$/.test(line.trim())) suppressEcho = true;
  });
  if (tray) { await tray.banner(); tray.render(); }
  else { output.write(`${banner}\n`); shell.setPrompt('roster> '); shell.prompt(); }
  try {
    for await (const line of shell) {
      if (handledLines.shift()) continue;
      const wasSecret = state.pendingSecret !== null;
      if (wasSecret) output.write('\n');
      try {
        if (!(await dispatch(line))) break;
      } catch (error) {
        if (!(error instanceof Error)) throw error;
        errors.write(`${error.message}\n`);
        if (error instanceof ChecksPermissionError) {
          exitCode = 1;
          break;
        }
      } finally {
        if (wasSecret) suppressEcho = false;
      }
      if (state.pendingSecret === null) {
        if (tray) tray.render();
        else { shell.setPrompt('roster> '); shell.prompt(); }
      }
    }
  } finally {
    shell.close();
    await Promise.all(sideRequests);
    tray?.close();
    terminalOutput.end();
    await history.flush().catch((error) => { historyError ??= error; });
  }
  if (historyError) exitCode = 1;
  return exitCode;
}
