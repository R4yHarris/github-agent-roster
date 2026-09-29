import { basename, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { Writable } from 'node:stream';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { submitAsk } from './lib/ask.mjs';
import { prepareBuiltinPublication, runBuiltinIssue } from './lib/builtin.mjs';
import {
  closeMergedIssue, issueMergeMessage, mergedPullNumber, mergedPullNumberFromFailure,
} from './lib/issue-board.mjs';
import { loadConfig, setConfigValue } from './lib/config.mjs';
import { recordEvaluation } from './lib/eval.mjs';
import { formatRecommendation, recommend, repositoryRoot, TASK_CLASSES } from './lib/learn.mjs';
import { formatMetrics, loadMetrics, summarizeMetrics } from './lib/metrics.mjs';
import { resolveContractsPath } from './lib/paths.mjs';
import { formatStatus, readStatus } from './lib/status.mjs';
import { createFileVault, validateSecretName } from './vault/file.mjs';
import { buildPublishEnv } from './metrics/run.mjs';

const rosterRoot = fileURLToPath(new URL('../', import.meta.url));
const help = `Commands:
  /ask TEXT                 Create an issue, or draft one if gh is unavailable
  /model [MODEL]            Show or persist the LLM model
  /effort [l|m|h|x]         Show or persist the effort level
  /run N [--auto-model]     Run builtin seats, optionally routing from human evaluations
  /status [N] [--offline]   Show an issue, open PR, and local worktree
  /eval TARGET VERDICT 1-5 y|n
  /publish [SUBJECT]        Publish reviewed changes (conventional subject)
  /stats [REF]              Show AI-Run metrics
  /recommend feat|fix|docs|test
  /vault [list]             List secret names
  /vault get NAME           Check whether a secret is stored, without revealing it
  /vault set NAME           Enter a secret with input hidden
  /help                     Show these commands
  /quit                     Exit
`;

class ChecksPermissionError extends Error {}

async function publishWithContracts({ contractsPath, cwd, env, message, output, errorOutput }) {
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
  const code = await main(['--message', message, '--merge-when-green'], {
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
  summarizeMetrics, formatMetrics, recommend, formatRecommendation,
  resolveContractsPath, prepareBuiltinPublication, createFileVault,
  validateSecretName, readStatus, formatStatus, setConfigValue,
  issueCloser: closeMergedIssue,
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
  config = loadConfig({ repoRoot }),
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
        state.config = await api.setConfigValue('model', args === 'clear' ? '' : args, { repoRoot });
        output.write(`Model: ${state.config.llm.model || '(unset)'}\n`);
        return true;
      }
      case 'effort': {
        if (!args) {
          output.write(`Effort: ${state.config.llm.effort}\n`);
          return true;
        }
        state.config = await api.setConfigValue('effort', args, { repoRoot });
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
          log: (message) => messages.push(message),
        });
        state.published = false;
        const command = state.lastRun.command;
        for (const message of messages) {
          output.write(`${typeof command === 'string' && command &&
            !command.endsWith(' --merge-when-green')
            ? message.replace(command, `${command} --merge-when-green`)
            : message}\n`);
        }
        output.write('Use /publish to publish reviewed changes with --merge-when-green.\n');
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
        const fields = args.split(/\s+/);
        if (fields.length !== 4) throw new TypeError('Use /eval TARGET accept|reject|rework 1-5 y|n.');
        const evaluation = await api.recordEvaluation(...fields, { cwd: currentRoot() });
        output.write(`Recorded AI-Eval for ${evaluation.sha ?? evaluation.session}.\n`);
        return true;
      }
      case 'publish': {
        if (state.lastRun && state.published) {
          throw new Error('This run was already published; start another /run before publishing again.');
        }
        const subject = args || (state.lastRun ? `feat: issue ${state.lastRun.issue.number}` : null);
        if (subject !== null) conventionalSubject(subject);
        const message = subject && state.lastRun
          ? issueMergeMessage(subject, state.lastRun.issue.number) : subject;
        const appId = Boolean(env.GITHUB_APP_ID);
        const keyPath = Boolean(env.GITHUB_APP_PRIVATE_KEY_PATH);
        if (appId !== keyPath) {
          throw new Error('Set both GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH to publish.');
        }
        if (!appId) {
          api.resolveContractsPath({ repoRoot, cwd, env });
          output.write(`From ${state.lastRun?.worktreePath ?? currentRoot()}, publish reviewed changes:\n` +
            `node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "${message ?? '<conventional subject>'}" --merge-when-green\n`);
          return true;
        }
        if (subject === null) throw new TypeError('Use /publish <conventional subject> or /run N first.');
        let contractsPath;
        let publishEnv;
        let publishRoot;
        if (state.lastRun) {
          ({ contractsPath, publishEnv, worktreePath: publishRoot } =
            await api.prepareBuiltinPublication(state.lastRun, { cwd, config: state.config, env }));
        } else {
          publishRoot = currentRoot();
          contractsPath = api.resolveContractsPath({ repoRoot: publishRoot, cwd, env });
          publishEnv = buildPublishEnv({ config: state.config, env });
          publishEnv.GITHUB_APP_PRIVATE_KEY_PATH = resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH);
        }
        const finishIssue = async (pullNumber) => {
          state.published = true;
          await api.issueCloser({
            issue: state.lastRun.issue, pullNumber,
            runLine: state.lastRun.runs?.coder?.line,
            repoRoot: state.lastRun.repoRoot, cwd, env,
          });
          output.write(`Commented on and closed issue #${state.lastRun.issue.number}.\n`);
        };
        let publication;
        try {
          publication = await api.publisher({
            contractsPath, cwd: publishRoot, env: publishEnv, message,
            output, errorOutput,
          });
        } catch (error) {
          if (state.lastRun && Number.isSafeInteger(error.mergedPullRequest) &&
              error.mergedPullRequest > 0) {
            await finishIssue(error.mergedPullRequest);
          }
          throw error;
        }
        if (state.lastRun) {
          if (!Number.isSafeInteger(publication?.mergedPullRequest) || publication.mergedPullRequest <= 0) {
            throw new Error('Publisher did not confirm a merged PR; issue remains open');
          }
          await finishIssue(publication.mergedPullRequest);
        } else state.published = true;
        return true;
      }
      case 'stats': {
        if (args && /\s/.test(args)) throw new TypeError('Use /stats [REF].');
        output.write(api.formatMetrics(api.summarizeMetrics(metrics(args))));
        return true;
      }
      case 'recommend': {
        if (!TASK_CLASSES.includes(args)) throw new TypeError('Use /recommend feat|fix|docs|test.');
        output.write(api.formatRecommendation(api.recommend(metrics(), args), args));
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
