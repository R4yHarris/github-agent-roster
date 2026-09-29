import { basename, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { Writable } from 'node:stream';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { writeAsk } from './lib/ask.mjs';
import { prepareBuiltinPublication, runBuiltinIssue } from './lib/builtin.mjs';
import { loadConfig } from './lib/config.mjs';
import { recordEvaluation } from './lib/eval.mjs';
import { formatRecommendation, recommend, repositoryRoot, TASK_CLASSES } from './lib/learn.mjs';
import { formatMetrics, loadMetrics, summarizeMetrics } from './lib/metrics.mjs';
import { resolveContractsPath } from './lib/paths.mjs';
import { createFileVault, validateSecretName } from './vault/file.mjs';

const rosterRoot = fileURLToPath(new URL('../', import.meta.url));
const help = `Commands:
  /ask TEXT                 Draft a local ask and task
  /run N                    Run the builtin planner and coder for issue N
  /status                   Show this shell's repository and last run
  /eval TARGET VERDICT 1-5 y|n
  /publish [SUBJECT]        Publish reviewed changes (conventional subject)
  /stats [REF]              Show AI-Run metrics
  /recommend feat|fix|docs|test
  /vault [list]             List secret names
  /vault set NAME           Enter a secret with input hidden
  /help                     Show these commands
  /quit                     Exit
`;

class ChecksPermissionError extends Error {}

async function publishWithContracts({ contractsPath, cwd, env, message, output, errorOutput }) {
  const { main } = await import(pathToFileURL(join(contractsPath, 'scripts', 'agent-pr.mjs')).href);
  let errorText = '';
  const stderr = {
    write(text) {
      errorText += String(text);
      errorOutput.write(text);
    },
  };
  const code = await main(['--message', message, '--merge-when-green'], {
    cwd, env, stdout: output, stderr,
  });
  if (code === 0) return;
  if (errorText.includes('HTTP 422')) {
    throw new ChecksPermissionError('Checks permission is not accepted on the installation.');
  }
  throw new Error('App publication failed; see the publisher error above.');
}

const defaultServices = {
  writeAsk, runBuiltinIssue, recordEvaluation, repositoryRoot, loadMetrics,
  summarizeMetrics, formatMetrics, recommend, formatRecommendation,
  resolveContractsPath, prepareBuiltinPublication, createFileVault,
  validateSecretName, publisher: publishWithContracts,
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
  const state = { lastAsk: null, lastRun: null, pendingSecret: null, published: false };
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
        const ask = await api.writeAsk(args, { repoRoot, config, env });
        state.lastAsk = ask;
        output.write(`Ask: ${ask.askPath}\nRECIPE: ${ask.recipePath}\nTASK: ${ask.taskPath}\n`);
        return true;
      }
      case 'run': {
        const issue = /^(?:--issue\s+)?([1-9]\d*)$/.exec(args);
        if (!issue) throw new TypeError('Use /run N or /run --issue N.');
        const messages = [];
        state.lastRun = await api.runBuiltinIssue(issue[1], {
          cwd, repoRoot, config, env, publish: false,
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
        if (args) throw new TypeError('Use /status.');
        output.write(`Repository: ${basename(currentRoot())}\nRuntime: builtin\n` +
          `LLM: ${config.llm.base_url || 'stub'}\n` +
          `Last run: ${state.lastRun?.task ?? 'none'}\n` +
          `Worktree: ${state.lastRun?.worktreePath ?? 'none'}\n` +
          `Published: ${state.published ? 'yes' : 'no'}\n`);
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
        const appId = Boolean(env.GITHUB_APP_ID);
        const keyPath = Boolean(env.GITHUB_APP_PRIVATE_KEY_PATH);
        if (appId !== keyPath) {
          throw new Error('Set both GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH to publish.');
        }
        if (!appId) {
          api.resolveContractsPath({ repoRoot, cwd, env });
          output.write(`From ${state.lastRun?.worktreePath ?? currentRoot()}, publish reviewed changes:\n` +
            `node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "${subject ?? '<conventional subject>'}" --merge-when-green\n`);
          return true;
        }
        if (subject === null) throw new TypeError('Use /publish <conventional subject> or /run N first.');
        let contractsPath;
        let publishEnv;
        let publishRoot;
        if (state.lastRun) {
          ({ contractsPath, publishEnv, worktreePath: publishRoot } =
            await api.prepareBuiltinPublication(state.lastRun, { cwd, config, env }));
        } else {
          publishRoot = currentRoot();
          contractsPath = api.resolveContractsPath({ repoRoot: publishRoot, cwd, env });
          publishEnv = { ...env, GITHUB_APP_PRIVATE_KEY_PATH: resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH) };
          delete publishEnv[config.llm.api_key_env];
        }
        await api.publisher({
          contractsPath, cwd: publishRoot, env: publishEnv, message: subject,
          output, errorOutput,
        });
        state.published = true;
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
        const secret = /^set\s+(\S+)$/.exec(args);
        if (!secret) throw new TypeError('Use /vault [list] or /vault set NAME.');
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
    banner: `${basename(api.repositoryRoot(cwd))} | runtime builtin | llm ${config.llm.base_url || 'stub'}`,
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
