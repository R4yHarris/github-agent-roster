#!/usr/bin/env node

import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runIssue } from './lib/issue.mjs';
import { recordEvaluation } from './lib/eval.mjs';
import { formatRecommendation, recommend, repositoryRoot, TASK_CLASSES } from './lib/learn.mjs';
import { submitAsk } from './lib/ask.mjs';
import { runBuiltinIssue } from './lib/builtin.mjs';
import { formatMetrics, loadMetrics, summarizeMetrics } from './lib/metrics.mjs';
import { resolveContractsPath } from './lib/paths.mjs';
import { validateRecipe } from './lib/recipe.mjs';
import { startRepl } from './repl.mjs';
import { createFileVault, validateSecretName } from './vault/file.mjs';

const rosterRoot = fileURLToPath(new URL('../', import.meta.url));
const help = `Usage:
  roster --help
  roster ask "..."
  roster run --issue N
  roster run --issue N --runtime builtin [--seats planner,coder] [--publish]
  roster run --issue N --runtime builtin --seats planner,coder
  roster recipe validate PATH
  roster stats [--ref REVISION_OR_RANGE] [--evals PATH]
  roster vault set NAME
  roster vault list
  roster vault get NAME
  roster eval <sha-or-session> <accept|reject|rework> <1-5> <y|n>
  roster recommend --task-class feat|fix|docs|test

Ask creates a GitHub issue when gh is available; otherwise it saves a local draft.
Bare run prepares the legacy worktree; builtin run
plans and executes planner then coder in one worktree. Publishing is opt-in.
Recipe validates strict v0 seat YAML. Stats joins contracts AI-Run history with local runs and human evals.
Eval records a human decision locally. Recommend needs at least 3 evaluated runs.
Vault set reads a secret from stdin; vault list prints names, never values.
`;

async function setVaultSecret(name) {
  validateSecretName(name);
  if (process.stdin.isTTY) throw new TypeError('Pipe the secret to roster vault set NAME through stdin.');
  process.stdin.setEncoding('utf8');
  let value = '';
  try {
    for await (const chunk of process.stdin) value += chunk;
  } catch {
    throw new Error('Unable to read the secret from stdin.');
  }
  await createFileVault().set(name, value.replace(/\r?\n$/, ''));
  process.stdout.write(`Stored secret ${name}.\n`);
}

async function getVaultSecret(name) {
  validateSecretName(name);
  if (process.stdout.isTTY) throw new TypeError('Pipe roster vault get NAME; refusing to print a secret in a terminal.');
  const value = await createFileVault().get(name);
  if (value === undefined) throw new Error(`No secret stored for ${name}.`);
  process.stdout.write(value);
}

function statsOptions(args) {
  const options = {};
  const seen = new Set();
  for (let index = 0; index < args.length; index += 2) {
    const flag = args[index];
    const value = args[index + 1];
    if (!['--ref', '--evals'].includes(flag) || !value || value.startsWith('--') || seen.has(flag)) {
      throw new TypeError('Use roster stats [--ref REVISION_OR_RANGE] [--evals PATH].');
    }
    seen.add(flag);
    options[flag === '--ref' ? 'ref' : 'evalsPath'] = value;
  }
  return options;
}

async function main(args) {
  if (args.length === 0 && process.stdin.isTTY) {
    process.exitCode = await startRepl({ repoRoot: rosterRoot });
  } else if (args.length === 0 || (args.length === 1 && ['--help', '-h'].includes(args[0]))) {
    process.stdout.write(help);
    if (args.length === 0) process.exitCode = 2;
  } else if (args.length === 2 && args[0] === 'ask') {
    const result = await submitAsk(args[1], {
      repoRoot: rosterRoot,
    });
    process.stdout.write(result.mode === 'issue'
      ? `Issue: ${result.url}\n`
      : `Ask: ${result.askPath}\nRECIPE: ${result.recipePath}\nTASK: ${result.taskPath}\nNext: ${result.command}\n`);
  } else if (args.length === 3 && args[0] === 'run' && args[1] === '--issue') {
    await runIssue(args[2]);
  } else if (args[0] === 'run' && args.includes('--runtime')) {
    const options = runOptions(args.slice(1));
    await runBuiltinIssue(options.issue, { publish: options.publish, seats: options.seats,
      repoRoot: rosterRoot });
  } else if (args.length === 3 && args[0] === 'recipe' && args[1] === 'validate') {
    validateRecipe(args[2]);
    process.stdout.write(`Valid recipe: ${args[2]}\n`);
  } else if (args.length === 3 && args[0] === 'vault' && args[1] === 'set') {
    await setVaultSecret(args[2]);
  } else if (args.length === 2 && args[0] === 'vault' && args[1] === 'list') {
    const names = await createFileVault().list();
    if (names.length) process.stdout.write(`${names.join('\n')}\n`);
  } else if (args.length === 3 && args[0] === 'vault' && args[1] === 'get') {
    await getVaultSecret(args[2]);
  } else if (args[0] === 'stats') {
    const options = statsOptions(args.slice(1));
    const records = loadMetrics({
      ...options,
      evalsPath: options.evalsPath === undefined ? undefined : resolve(options.evalsPath),
      contractsPath: resolveContractsPath(),
      cwd: repositoryRoot(),
    });
    process.stdout.write(formatMetrics(summarizeMetrics(records)));
  } else if (args[0] === 'eval') {
    if (args.length !== 5) {
      throw new TypeError('Use roster eval <sha-or-session> <accept|reject|rework> <1-5> <y|n>.');
    }
    const evaluation = await recordEvaluation(...args.slice(1));
    process.stdout.write(`Recorded AI-Eval for ${evaluation.sha ?? evaluation.session}.\n`);
  } else if (args[0] === 'recommend') {
    if (args.length !== 3 || args[1] !== '--task-class' || !TASK_CLASSES.includes(args[2])) {
      throw new TypeError('Use roster recommend --task-class feat|fix|docs|test.');
    }
    const records = loadMetrics({ contractsPath: resolveContractsPath(), cwd: repositoryRoot() });
    process.stdout.write(formatRecommendation(recommend(records, args[2]), args[2]));
  } else {
    throw new TypeError('Unknown arguments. Run roster --help for usage.');
  }

  function runOptions(args) {
    const options = {};
    const seen = new Set();
    const usage = 'Use roster run --issue N --runtime builtin [--seats planner,coder] [--publish].';
    for (let index = 0; index < args.length; index += 1) {
      const flag = args[index];
      if (!['--issue', '--seat', '--seats', '--runtime', '--publish'].includes(flag) || seen.has(flag)) {
        throw new TypeError(usage);
      }
      seen.add(flag);
      if (flag === '--publish') options.publish = true;
      else {
        const value = args[++index];
        if (!value || value.startsWith('--')) throw new TypeError(usage);
        options[flag.slice(2)] = value;
      }
    }
    if (!options.issue || options.runtime !== 'builtin' ||
        (options.seat !== undefined && options.seat !== 'coder') ||
        (options.seats !== undefined && options.seats !== 'planner,coder') ||
        (options.seat !== undefined && options.seats !== undefined)) {
      throw new TypeError(usage);
    }
    return { ...options, seats: 'planner,coder' };
  }
}

try {
  await main(process.argv.slice(2));
} catch (error) {
  if (!(error instanceof Error)) throw error;
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
