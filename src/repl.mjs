import { execFileSync } from 'node:child_process';
import { lstatSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { Writable } from 'node:stream';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { formatAsk, submitAsk } from './lib/ask.mjs';
import { prepareBuiltinPublication, runBuiltinAsk, runBuiltinIssue } from './lib/builtin.mjs';
import {
  commentMergedIssue, mergedPullNumber, mergedPullNumberFromFailure,
} from './lib/issue-board.mjs';
import { isReviewRequired, loadConfig, requirePublicationEnabled, setConfigValue } from './lib/config.mjs';
import { parseEvaluationArgs, recordEvaluation } from './lib/eval.mjs';
import { parseRecommendationArgs, repositoryRoot } from './lib/learn.mjs';
import { formatMetrics, loadAvailableMetrics, loadMetrics, summarizeMetrics } from './lib/metrics.mjs';
import { resolveContractsPath, resolveProjectRoot } from './lib/paths.mjs';
import { formatRoute, routeTask } from './lib/route.mjs';
import {
  buildPublishMessage, formatPublishCommand, formatPublishEnvironment, parsePublishArgs, publicationTask,
} from './lib/publication.mjs';
import { redactEvidence } from './runtime/excellence.mjs';
import { requirePassingReview } from './seats/reviewer.mjs';
import { isLlmTimeout } from './llm/request.mjs';
import { formatStatus, readStatus } from './lib/status.mjs';
import { createFileVault, validateSecretName } from './vault/file.mjs';
import { buildPublishEnv, resolvePublishModel } from './metrics/run.mjs';
import { humanEvalHint } from './lib/seat-publication.mjs';
import { readIssueLogs } from './lib/run-log.mjs';
import { createDebugLog } from './lib/debug-log.mjs';
import { createTray } from './shell/tray.mjs';

const rosterRoot = fileURLToPath(new URL('../', import.meta.url));
const help = `Commands:
  TEXT                      Run a direct local ask without creating an issue
  /ask TEXT                 Create an issue, or draft one if gh is unavailable
  /model [MODEL]            Show or persist the LLM model
  /effort [l|m|h|x|none]    Show or persist an explicit effort override
  /run N [--auto-model] [--confirm]  Summarize the task and continue; --confirm pauses
  /status [N] [--offline]   Show an issue, open PR, and local worktree
  /log N                   Tail local issue seat logs without network access
  /debug on|off             Toggle process-only testing metadata logs
  /log debug                Tail this process's debug file
  /statusbar on|off          Toggle both delivery-tray bars for this process
  /eval TARGET VERDICT 1-5 y|n [--minutes N] [--comment "TEXT"]
  /publish [SUBJECT] [--model MODEL] [--skip-review]  Publish reviewed seat or GHCP changes
  /stats [REF]              Show AI-Run metrics
  /recommend feat|fix|docs|test [--difficulty 1-5]
  /vault [list]             List secret names
  /vault get NAME           Check whether a secret is stored, without revealing it
  /vault set NAME           Enter a secret with input hidden
  /help                     Show these commands
  /quit                     Exit
`;

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

const defaultServices = {
  submitAsk, runBuiltinAsk, runBuiltinIssue, recordEvaluation, repositoryRoot, loadMetrics,
  summarizeMetrics, formatMetrics, loadAvailableMetrics, routeTask, formatRoute,
  resolveContractsPath, prepareBuiltinPublication, createFileVault,
  validateSecretName, readStatus, formatStatus, setConfigValue,
  publicationTask,
  readIssueLogs,
  issueCommenter: commentMergedIssue,
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
} = {}) {
  const api = { ...defaultServices, ...services };
  const state = { lastAsk: null, lastRun: null, pendingSecret: null, published: false, config, debug,
    statusbar: true, display: {
      issue: null, branch: api.repositoryBranch(cwd), seat: config.seat.id, state: 'idle', busy: false,
      model: config.llm.model, host: config.llm.base_url ? new URL(config.llm.base_url).host : '',
      effort: config.llm.effort, contextUsed: undefined, contextMax: config.llm.context_max, startedAt: null,
    } };
  const notify = () => onStateChange(state);
  const receiveEvent = (event) => {
    const display = state.display;
    display.seat = event.seat;
    if (event.type === 'seat-start') {
      display.state = { planner: 'planning', coder: 'drafting', reviewer: 'reviewing' }[event.seat];
      Object.assign(display, { model: event.model, host: event.host, effort: event.effort,
        contextMax: event.contextMax, contextUsed: undefined });
    } else if (event.type === 'tool' && event.name === 'run_test') display.state = 'testing';
    else if (event.type === 'http' && event.phase === 'start') {
      display.state = { planner: 'planning', coder: 'drafting', reviewer: 'reviewing' }[event.seat];
      if (event.effort !== undefined) display.effort = event.effort;
    } else if (event.type === 'seat-error') display.state = 'failed';
    else if (event.type === 'seat-end') {
      if (event.model) display.model = event.model;
      display.contextUsed = event.contextUsed;
      if (event.verdict) display.state = event.verdict === 'pass' ? 'passed' : 'failed';
    }
    notify();
  };
  const currentRoot = () => state.lastRun?.repoRoot ?? api.repositoryRoot(cwd);
  const metrics = (ref) => api.loadMetrics({
    contractsPath: api.resolveContractsPath({ repoRoot, cwd, env }),
    cwd: currentRoot(),
    ...(ref ? { ref } : {}),
  });

  async function executeRun(run, options = {}) {
    state.lastRun = { planningOnly: true, failed: true, askKind: 'clarify', command: null };
    state.published = false;
    Object.assign(state.display, { issue: options.issue ?? null,
      branch: options.issue ? `issue-${options.issue}` : state.display.branch,
      seat: 'planner', state: 'planning', busy: true, startedAt: Date.now(), contextUsed: undefined });
    notify();
    try {
      state.lastRun = await run({
        cwd, repoRoot, config: state.config, env, publish: false, debug: state.debug, ...options,
        log: (message) => output.write(`${message}\n`), errorOutput,
        onRunEvent: receiveEvent,
      });
    } catch (error) {
      if (isLlmTimeout(error)) {
        state.lastRun = { planningOnly: true, failed: true, timedOut: true, command: null };
      }
      state.display.state = 'failed';
      throw error;
    } finally {
      state.display.busy = false;
      notify();
    }
    if (state.lastRun.task) state.display.branch = state.lastRun.task;
    state.display.state = state.lastRun.review
      ? state.lastRun.review.verdict === 'pass' ? 'passed' : 'failed'
      : state.lastRun.failed ? 'failed' : 'idle';
    notify();
    output.write(state.lastRun.askKind === 'clarify'
      ? `${state.lastRun.clarification}\n`
      : state.lastRun.planPath
      ? 'PLAN ready; review the child drafts and run bounded slices. No coder ran.\n'
      : state.lastRun.planningOnly
      ? 'Paused by --confirm; review TASK.md before running without --confirm.\n'
      : state.lastRun.failed
      ? 'Planning failed; stubs are unverified and publication is disabled. Fix the endpoint output, then retry.\n'
      : 'Use /publish to publish reviewed changes with --merge-when-green.\n');
    return true;
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
    if (!text) return true;
    if (!text.startsWith('/')) return executeRun((options) => api.runBuiltinAsk(text, options));
    const match = /^\/([a-z]+)(?:\s+(.*))?$/.exec(text);
    if (!match) {
      errorOutput.write(`Unknown command: ${text.split(/\s+/)[0]}. Type /help.\n`);
      return true;
    }
    const [, command, rawArguments] = match;
    const args = rawArguments?.trim() ?? '';
    switch (command) {
      case 'statusbar':
        if (!['on', 'off'].includes(args)) throw new TypeError('Use /statusbar on or /statusbar off.');
        state.statusbar = args === 'on';
        notify();
        output.write(`Status bars ${args}.\n`);
        return true;
      case 'debug': {
        if (!['on', 'off'].includes(args)) throw new TypeError('Use /debug on or /debug off.');
        state.debug.setEnabled(args === 'on');
        notify();
        output.write(`Debug logging ${args}.\n`);
        return true;
      }
      case 'ask': {
        if (!args) throw new TypeError('Use /ask TEXT.');
        const ask = await api.submitAsk(args, { cwd, repoRoot, config: state.config, env });
        state.lastAsk = ask;
        output.write(formatAsk(ask));
        return true;
      }
      case 'model': {
        if (!args) {
          output.write(`Model: ${state.config.llm.model || '(unset)'}\n`);
          return true;
        }
        state.config = await api.setConfigValue('model', args === 'clear' ? '' : args, { repoRoot, cwd });
        state.display.model = state.config.llm.model;
        notify();
        output.write(`Model: ${state.config.llm.model || '(unset)'}\n`);
        return true;
      }
      case 'effort': {
        if (!args) {
          output.write(`Effort: ${state.config.llm.effort}\n`);
          return true;
        }
        state.config = await api.setConfigValue('effort', args, { repoRoot, cwd });
        state.display.effort = state.config.llm.effort;
        notify();
        output.write(`Effort: ${state.config.llm.effort}\n`);
        return true;
      }
      case 'run': {
        const issue = /^(?:--issue\s+)?([1-9]\d*)((?:\s+--[a-z-]+)*)$/.exec(args);
        const flags = issue?.[2].trim().split(/\s+/).filter(Boolean) ?? [];
        if (!issue || flags.some((flag) => !['--auto-model', '--confirm'].includes(flag)) ||
            new Set(flags).size !== flags.length) {
          throw new TypeError('Use /run N [--auto-model] [--confirm] or /run --issue N [--auto-model] [--confirm].');
        }
        return executeRun((options) => api.runBuiltinIssue(issue[1], options), {
          autoModel: flags.includes('--auto-model'), confirm: flags.includes('--confirm'), issue: Number(issue[1]),
        });
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
        output.write(api.formatStatus(await api.readStatus({
          issue, offline, cwd: currentRoot(), config: state.config,
        })));
        return true;
      }
      case 'eval': {
        const { values, options } = parseEvaluationArgs(args);
        const evaluation = await api.recordEvaluation(...values, { ...options, cwd: currentRoot(), env });
        output.write(`Recorded AI-Eval for ${evaluation.sha ?? evaluation.session}.\n`);
        return true;
      }
      case 'log': {
        if (args === 'debug') {
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
            issueNumber: state.lastRun?.issue?.number,
            seats: state.lastRun
              ? `planner, coder, reviewer (${reviewLabel})`
              : undefined,
            ghcp,
          });
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
          seats: state.lastRun
            ? `planner, coder, reviewer (${reviewLabel})`
            : undefined,
          ghcp: !state.lastRun,
        });
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
        return true;
      }
      case 'recommend': {
        const { taskClass, difficulty } = parseRecommendationArgs(['--task-class', ...args.split(/\s+/)]);
        const root = state.lastRun?.repoRoot ?? resolveProjectRoot(cwd);
        const route = await api.routeTask({ cwd: root, installationRoot: repoRoot, taskClass,
          difficulty: difficulty ?? 2, records: api.loadAvailableMetrics({ cwd: root }) });
        output.write(api.formatRoute(route, taskClass, state.config, env));
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
        if (args) throw new TypeError('Use /help.');
        output.write(help);
        return true;
      case 'quit':
        if (args) throw new TypeError('Use /quit.');
        return false;
      default:
        errorOutput.write(`Unknown command: /${command}. Type /help.\n`);
        return true;
    }
  }

  return {
    dispatch,
    state,
    banner: 'github-agent-roster',
  };
}

export async function startRepl({
  input = process.stdin,
  output = process.stdout,
  errorOutput = process.stderr,
  ...options
} = {}) {
  let tray;
  const terminal = input.isTTY === true;
  const messages = { write(text) { if (tray) tray.write(text); else output.write(text); } };
  const errors = { write(text) { if (tray) tray.write(text, errorOutput); else errorOutput.write(text); } };
  const { dispatch, state, banner } = createDispatcher({ ...options, output: messages, errorOutput: errors,
    onStateChange() { if (tray) tray.render(); } });
  let suppressEcho = false;
  const terminalOutput = new Writable({
    write(chunk, encoding, callback) {
      if (!suppressEcho && state.pendingSecret === null) output.write(chunk, encoding);
      callback();
    },
  });
  terminalOutput.isTTY = terminal;
  terminalOutput.columns = output.columns ?? 80;
  const shell = createInterface({ input, output: terminalOutput, terminal, historySize: 0 });
  if (terminal) tray = createTray({ output, state, shell });
  shell.on('SIGINT', () => shell.close());
  shell.on('line', (line) => {
    tray?.committed();
    if (suppressEcho) suppressEcho = false;
    else if (/^\/vault set [A-Za-z_][A-Za-z0-9_]{0,63}$/.test(line.trim())) suppressEcho = true;
  });
  if (tray) { tray.banner(); tray.render(); }
  else { output.write(`${banner}\n`); shell.setPrompt('roster> '); shell.prompt(); }
  let exitCode = 0;
  try {
    for await (const line of shell) {
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
    tray?.close();
    terminalOutput.end();
  }
  return exitCode;
}
