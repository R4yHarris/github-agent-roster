import { basename, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { Writable } from 'node:stream';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { submitAsk } from './lib/ask.mjs';
import { prepareBuiltinPublication, runBuiltinIssue } from './lib/builtin.mjs';
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
import { formatStatus, readStatus } from './lib/status.mjs';
import { createFileVault, validateSecretName } from './vault/file.mjs';
import { buildPublishEnv, resolvePublishModel } from './metrics/run.mjs';
import { humanEvalHint } from './lib/seat-publication.mjs';

const rosterRoot = fileURLToPath(new URL('../', import.meta.url));
const help = `Commands:
  /ask TEXT                 Create an issue, or draft one if gh is unavailable
  /model [MODEL]            Show or persist the LLM model
  /effort [l|m|h|x]         Show or persist the effort level
  /run N [--auto-model]     Run builtin seats, optionally routing from human evaluations
  /status [N] [--offline]   Show an issue, open PR, and local worktree
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
  submitAsk, runBuiltinIssue, recordEvaluation, repositoryRoot, loadMetrics,
  summarizeMetrics, formatMetrics, loadAvailableMetrics, routeTask, formatRoute,
  resolveContractsPath, prepareBuiltinPublication, createFileVault,
  validateSecretName, readStatus, formatStatus, setConfigValue,
  publicationTask,
  issueCommenter: commentMergedIssue,
  publisher: publishWithContracts,
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
} = {}) {
  const api = { ...defaultServices, ...services };
  const state = { lastAsk: null, lastRun: null, pendingSecret: null, published: false, config };
  const currentRoot = () => state.lastRun?.repoRoot ?? api.repositoryRoot(cwd);
  const metrics = (ref) => api.loadMetrics({
    contractsPath: api.resolveContractsPath({ repoRoot, cwd, env }),
    cwd: currentRoot(),
    ...(ref ? { ref } : {}),
  });

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
    const match = /^\/([a-z]+)(?:\s+(.*))?$/.exec(text);
    if (!match) {
      errorOutput.write(`Unknown command: ${text.split(/\s+/)[0]}. Type /help.\n`);
      return true;
    }
    const [, command, rawArguments] = match;
    const args = rawArguments?.trim() ?? '';
    switch (command) {
      case 'ask': {
        if (!args) throw new TypeError('Use /ask TEXT.');
        const ask = await api.submitAsk(args, { cwd, repoRoot, config: state.config, env });
        state.lastAsk = ask;
        output.write(ask.mode === 'issue'
          ? `Issue: ${ask.url}\n`
          : `Ask: ${ask.askPath}\nRECIPE: ${ask.recipePath}\nTASK: ${ask.taskPath}\nNext: ${ask.command}\n`);
        return true;
      }
      case 'model': {
        if (!args) {
          output.write(`Model: ${state.config.llm.model || '(unset)'}\n`);
          return true;
        }
        state.config = await api.setConfigValue('model', args === 'clear' ? '' : args, { repoRoot, cwd });
        output.write(`Model: ${state.config.llm.model || '(unset)'}\n`);
        return true;
      }
      case 'effort': {
        if (!args) {
          output.write(`Effort: ${state.config.llm.effort}\n`);
          return true;
        }
        state.config = await api.setConfigValue('effort', args, { repoRoot, cwd });
        output.write(`Effort: ${state.config.llm.effort}\n`);
        return true;
      }
      case 'run': {
        const issue = /^(?:--issue\s+)?([1-9]\d*)(?:\s+--auto-model)?$/.exec(args);
        if (!issue) throw new TypeError('Use /run N [--auto-model] or /run --issue N [--auto-model].');
        const autoModel = args.endsWith(' --auto-model');
        const messages = [];
        state.lastRun = await api.runBuiltinIssue(issue[1], {
          cwd, repoRoot, config: state.config, env, publish: false, autoModel,
          log: (message) => messages.push(message), errorOutput,
        });
        state.published = false;
        const command = state.lastRun.command;
        for (const message of messages) {
          output.write(`${typeof command === 'string' && command &&
            !command.endsWith(' --merge-when-green')
            ? message.replace(command, `${command} --merge-when-green`)
            : message}\n`);
        }
        output.write(state.lastRun.failed
          ? 'Planning failed; stubs are unverified and publication is disabled. Fix the endpoint output, then retry /run.\n'
          : 'Use /publish to publish reviewed changes with --merge-when-green.\n');
        return true;
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
      case 'publish': {
        requirePublicationEnabled(state.config);
        if (state.lastRun && state.published) {
          throw new Error('This run was already published; start another /run before publishing again.');
        }
        const { skipReview, subject: requestedSubject, model: requestedModel } = parsePublishArgs(args);
        const reviewBypass = skipReview || !isReviewRequired(state.config);
        const reviewLabel = skipReview ? 'gate bypassed with --skip-review'
          : !isReviewRequired(state.config) ? 'gate not required by configuration' : 'pass';
        const subject = requestedSubject || (state.lastRun ? `feat: issue ${state.lastRun.issue.number}` : null);
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
    banner: `${basename(api.repositoryRoot(cwd))} | seat ${config.seat.id} | runtime builtin | llm ${config.llm.base_url || 'stub'}`,
  };
}

export async function startRepl({
  input = process.stdin,
  output = process.stdout,
  errorOutput = process.stderr,
  ...options
} = {}) {
  const { dispatch, state, banner } = createDispatcher({ ...options, output, errorOutput });
  let suppressEcho = false;
  const terminal = input.isTTY === true;
  const terminalOutput = new Writable({
    write(chunk, encoding, callback) {
      if (!suppressEcho && state.pendingSecret === null) output.write(chunk, encoding);
      callback();
    },
  });
  terminalOutput.isTTY = terminal;
  terminalOutput.columns = output.columns ?? 80;
  const shell = createInterface({ input, output: terminalOutput, terminal, historySize: 0 });
  shell.on('SIGINT', () => shell.close());
  shell.on('line', (line) => {
    if (suppressEcho) suppressEcho = false;
    else if (/^\/vault set [A-Za-z_][A-Za-z0-9_]{0,63}$/.test(line.trim())) suppressEcho = true;
  });
  output.write(`${banner}\n`);
  shell.setPrompt('roster> ');
  shell.prompt();
  let exitCode = 0;
  try {
    for await (const line of shell) {
      const wasSecret = state.pendingSecret !== null;
      if (wasSecret) output.write('\n');
      try {
        if (!(await dispatch(line))) break;
      } catch (error) {
        if (!(error instanceof Error)) throw error;
        errorOutput.write(`${error.message}\n`);
        if (error instanceof ChecksPermissionError) {
          exitCode = 1;
          break;
        }
      } finally {
        if (wasSecret) suppressEcho = false;
      }
      if (state.pendingSecret === null) {
        shell.setPrompt('roster> ');
        shell.prompt();
      }
    }
  } finally {
    shell.close();
    terminalOutput.end();
  }
  return exitCode;
}
