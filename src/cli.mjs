#!/usr/bin/env node

import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runIssue } from './lib/issue.mjs';
import { parseEvaluationArgs, recordEvaluation } from './lib/eval.mjs';
import { parseRecommendationArgs, repositoryRoot } from './lib/learn.mjs';
import { formatAsk, submitAsk } from './lib/ask.mjs';
import { runBuiltinIssue, runBuiltinTask } from './lib/builtin.mjs';
import { loadConfig } from './lib/config.mjs';
import { runDemo } from './lib/demo.mjs';
import { checkDoctor, formatDoctor } from './lib/doctor.mjs';
import { formatInit, initializeRoster } from './lib/init.mjs';
import { runOnboard } from './onboard/wizard.mjs';
import { runFleet } from './lib/fleet-cli.mjs';
import { formatMetrics, loadAvailableMetrics, loadMetrics, summarizeMetrics } from './lib/metrics.mjs';
import { resolveContractsPath, resolveProjectRoot } from './lib/paths.mjs';
import { formatRoute, routeTask } from './lib/route.mjs';
import { formatStatus, readStatus } from './lib/status.mjs';
import { validateRecipe } from './lib/recipe.mjs';
import { startRepl } from './repl.mjs';
import { createFileVault, validateSecretName } from './vault/file.mjs';

const rosterRoot = fileURLToPath(new URL('../', import.meta.url));
const help = `Usage:
  roster                     Open the interactive shell in a TTY
  roster --help
  roster doctor
  roster init
  roster onboard
  roster fleet list
  roster fleet assist
  roster fleet add --id NAME --base-url URL [--model MODEL] [--context N] [--concurrency N] [--hardware TEXT] [--task-class feat,fix,docs,test]
  roster fleet probe ID [--set-model [MODEL]]
  roster fleet default ID
  roster fleet remove ID
  roster ask "..."
  roster run --issue N [--runtime builtin] [--seats planner,coder,reviewer] [--auto-model] [--publish] [--skip-review]
  roster run --seat coder --runtime builtin
  roster prepare --issue N
  roster run --ask-file PATH --runtime builtin
  roster status [--issue N] [--offline]
  roster recipe validate PATH
  roster stats [--ref REVISION_OR_RANGE] [--evals PATH]
  roster vault set NAME
  roster vault list
  roster vault get NAME
  roster eval <sha-or-session> <accept|reject|rework> <1-5> <y|n> [--minutes N] [--comment "TEXT"]
  roster recommend --task-class feat|fix|docs|test [--difficulty 1-5]

Ask creates a GitHub issue when gh is available; otherwise it saves a local draft.
Doctor checks local prerequisites without contacting GitHub or printing App env values.
Init copies review-only examples; it never overwrites agent-policy.yml.
Onboard configures a real vLLM model and local permissions in an interactive terminal.
Run classifies clarify | slice | feature | initiative before seats.
Slices run planner, coder, then reviewer; features and initiatives write PLAN.md only.
Run --seat coder without --issue executes an existing TASK.md in the current worktree.
Prepare creates a manual handoff without executing seats. Publishing is opt-in.
Recipe validates strict v0 seat YAML. Stats joins contracts AI-Run history with local runs and human evals.
Eval records a human decision locally. Recommend uses fleet priors, then qualifying human evidence.
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
    process.stdout.write(formatAsk(result));
  } else if (args.length === 1 && args[0] === 'doctor') {
    const result = checkDoctor();
    process.stdout.write(formatDoctor(result));
    if (!result.ok) process.exitCode = 1;
  } else if (args.length === 1 && args[0] === 'init') {
    process.stdout.write(formatInit(await initializeRoster()));
  } else if (args.length === 1 && args[0] === 'onboard') {
    process.exitCode = (await runOnboard({ installationRoot: rosterRoot })).exitCode;
  } else if (args[0] === 'fleet') {
    const result = await runFleet(args.slice(1), { installationRoot: rosterRoot });
    if (result.exitCode !== undefined) process.exitCode = result.exitCode;
  } else if (args[0] === 'prepare') {
    if (args.length !== 3 || args[1] !== '--issue') {
      throw new TypeError('Use roster prepare --issue N.');
    }
    await runIssue(args[2]);
  } else if (args[0] === 'run' && args.includes('--ask-file')) {
    const flags = args.slice(1);
    if (flags.length !== 4 ||
        !['--ask-file', '--runtime'].includes(flags[0]) ||
        !['--ask-file', '--runtime'].includes(flags[2]) ||
        flags[0] === flags[2]) {
      throw new TypeError('Use roster run --ask-file PATH --runtime builtin');
    }
    const options = { [flags[0]]: flags[1], [flags[2]]: flags[3] };
    if (!options['--ask-file'] || options['--runtime'] !== 'builtin') {
      throw new TypeError('Use roster run --ask-file PATH --runtime builtin');
    }
    const demo = await runDemo({ askFile: options['--ask-file'], repoRoot: rosterRoot });
    process.stdout.write(`Ask kind: ${demo.askKind}\n` +
      (demo.clarification ? `${demo.clarification}\n`
        : `Worktree: ${demo.worktreePath}\n` + (demo.planPath ? `PLAN: ${demo.planPath}\n`
          : `RECIPE: ${demo.recipePath}\nTASK: ${demo.taskPath}\nRESULT: ${demo.resultPath}\nREVIEW: ${demo.reviewPath}\n`)) +
      `Mode: ${demo.mode}\n`);
  } else if (args[0] === 'run') {
    const options = runOptions(args.slice(1));
    if (options.issue === undefined) await runBuiltinTask({ repoRoot: rosterRoot });
    else {
      const result = await runBuiltinIssue(options.issue, { publish: options.publish, seats: options.seats,
        skipReview: options.skipReview,
        autoModel: options.autoModel, repoRoot: rosterRoot });
      if (result.failed) process.exitCode = 1;
    }
  } else if (args[0] === 'status') {
    let issue;
    let offline = false;
    const flags = new Set();
    for (let index = 1; index < args.length; index += 1) {
      const flag = args[index];
      if (!['--issue', '--offline'].includes(flag) || flags.has(flag)) {
        throw new TypeError('Use roster status [--issue N] [--offline].');
      }
      flags.add(flag);
      if (flag === '--offline') offline = true;
      else {
        issue = args[++index];
        if (!issue || issue.startsWith('--')) {
          throw new TypeError('Use roster status [--issue N] [--offline].');
        }
      }
    }
    process.stdout.write(formatStatus(await readStatus({ issue, offline,
      config: loadConfig({ repoRoot: rosterRoot, cwd: process.cwd() }) })));
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
    const { values, options } = parseEvaluationArgs(args.slice(1));
    const evaluation = await recordEvaluation(...values, options);
    process.stdout.write(`Recorded AI-Eval for ${evaluation.sha ?? evaluation.session}.\n`);
  } else if (args[0] === 'recommend') {
    const { taskClass, difficulty } = parseRecommendationArgs(args.slice(1));
    const cwd = resolveProjectRoot();
    const route = await routeTask({ cwd, installationRoot: rosterRoot, taskClass,
      difficulty: difficulty ?? 2, records: loadAvailableMetrics({ cwd }) });
    process.stdout.write(formatRoute(route, taskClass, loadConfig({ cwd })));
  } else {
    throw new TypeError('Unknown arguments. Run roster --help for usage.');
  }

  function runOptions(args) {
    const options = {};
    const seen = new Set();
    const usage = 'Use roster run --issue N [--runtime builtin] [--seats planner,coder,reviewer] ' +
      '[--auto-model] [--publish] [--skip-review], ' +
      'or roster run --seat coder --runtime builtin for an existing TASK.md.';
    for (let index = 0; index < args.length; index += 1) {
      const flag = args[index];
      if (!['--issue', '--seat', '--seats', '--runtime', '--auto-model', '--publish', '--skip-review'].includes(flag) ||
          seen.has(flag)) {
        throw new TypeError(usage);
      }
      seen.add(flag);
      if (flag === '--publish') options.publish = true;
      else if (flag === '--auto-model') options.autoModel = true;
      else if (flag === '--skip-review') options.skipReview = true;
      else {
        const value = args[++index];
        if (!value || value.startsWith('--')) throw new TypeError(usage);
        options[flag.slice(2)] = value;
      }
    }
    if (options.issue === undefined && options.seat === 'coder' && options.runtime === 'builtin' &&
        options.seats === undefined && !options.autoModel && !options.publish && !options.skipReview) return options;
    if (!options.issue || (options.runtime !== undefined && options.runtime !== 'builtin') ||
        (options.seat !== undefined && options.seat !== 'coder') ||
        (options.seats !== undefined &&
          !['planner,coder', 'planner,coder,reviewer'].includes(options.seats)) ||
        (options.seat !== undefined && options.seats !== undefined)) {
      throw new TypeError(usage);
    }
    return { ...options, seats: 'planner,coder,reviewer' };
  }
}

try {
  await main(process.argv.slice(2));
} catch (error) {
  if (!(error instanceof Error)) throw error;
  process.stderr.write(`${error.message}\n`);
  if (error.result?.resultPath) process.stderr.write(`RESULT: ${error.result.resultPath}\n`);
  if (error.result?.review?.reviewPath) process.stderr.write(`REVIEW: ${error.result.review.reviewPath}\n`);
  process.exitCode = 1;
}
