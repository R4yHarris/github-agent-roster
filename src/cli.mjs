#!/usr/bin/env node

import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';

const cliStartedAt = performance.now();

const rosterRoot = fileURLToPath(new URL('../', import.meta.url));
const help = `Usage:
  roster                     Open the interactive shell in a TTY
  roster --debug [COMMAND]   Enable process-only debug JSONL logging
  roster --help
  roster doctor [--warm]
  roster init
  roster onboard [--discover]
  roster fleet list
  roster fleet assist
  roster fleet add --id NAME --base-url URL [--model MODEL] [--context N] [--concurrency N] [--hardware TEXT] [--task-class feat,fix,docs,test]
  roster fleet probe ID [--set-model [MODEL]]
  roster fleet default ID
  roster fleet remove ID
  roster ask "..."
  roster run --issue N [--runtime builtin] [--auto-model] [--seats planner,coder,reviewer] [--parallel K] [--saved] [--publish] [--skip-review] [--confirm] [--plan]
  roster run --seat coder --runtime builtin
  roster prepare --issue N
  roster run --ask-file PATH --runtime builtin
  roster history list [--store DIR] [--repo HASH] [--issue N] [--seat NAME] [--model MODEL] [--outcome VALUE] [--since TIME] [--until TIME]
  roster history show <session-or-run-or-record-id> [--store DIR]
  roster bench
  roster clean [--target issue|repo|machine-history|curated-memory] [--issue N] [--store DIR] [--scope SCOPE] [--state-root PATH] [--exclude NAME]... [--execute --yes]
  roster status [--issue N] [--offline]
  roster recipe validate PATH
  roster stats [--delivery [--json]] [--ref REVISION_OR_RANGE] [--evals PATH]
  roster vault set NAME
  roster vault list
  roster vault get NAME
  roster eval <sha-or-session> <accept|reject|rework> <1-5> <y|n> [--minutes N] [--comment "TEXT"]
  roster recommend --task-class feat|fix|docs|test [--difficulty 1-5]

Ask creates a GitHub issue when gh is available; otherwise it saves a local draft.
Doctor checks local prerequisites without contacting GitHub or printing App env values.
Doctor --warm additionally probes the configured LLM /models with its request timeout.
Init copies review-only examples; it never overwrites agent-policy.yml.
Onboard configures a real vLLM model and local permissions in an interactive terminal.
Run classifies clarify | slice | feature | initiative before seats.
Slices run planner, coder, then reviewer; features and initiatives write PLAN.md only.
Slices print outcome, allowed files, checks, and effort before continuing. Only --confirm pauses.
Run --seat coder without --issue executes an existing TASK.md in the current worktree.
Prepare creates a manual handoff without executing seats. Publishing is opt-in.
Recipe validates strict v0 seat YAML. Stats joins contracts AI-Run history with local runs and human evals.
Eval records a human decision locally. Recommend uses fleet priors, then qualifying human evidence.
Vault set reads a secret from stdin; vault list prints names, never values.
`;

async function setVaultSecret(name) {
  const { createFileVault, validateSecretName } = await import('./vault/file.mjs');
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
  const { createFileVault, validateSecretName } = await import('./vault/file.mjs');
  validateSecretName(name);
  if (process.stdout.isTTY) throw new TypeError('Pipe roster vault get NAME; refusing to print a secret in a terminal.');
  const value = await createFileVault().get(name);
  if (value === undefined) throw new Error(`No secret stored for ${name}.`);
  process.stdout.write(value);
}

async function main(args) {
  const debugFlag = args[0] === '--debug';
  if (debugFlag) args = args.slice(1);
  if ((args.length === 0 && !process.stdin.isTTY) || (args.length === 1 && ['--help', '-h'].includes(args[0]))) {
    process.stdout.write(help);
    if (args.length === 0) process.exitCode = 2;
    return;
  }
  const { createDebugLog } = await import('./lib/debug-log.mjs');
  const debug = createDebugLog({ enabled: debugFlag || process.env.ROSTER_DEBUG === '1' });
  if (args.length === 0) {
    const { startRepl } = await import('./repl.mjs');
    process.exitCode = await startRepl({ repoRoot: rosterRoot, debug });
  } else if (args.length === 2 && args[0] === 'ask') {
    const { formatAsk, submitAsk } = await import('./lib/ask.mjs');
    const result = await submitAsk(args[1], {
      repoRoot: rosterRoot,
    });
    process.stdout.write(formatAsk(result));
  } else if (args[0] === 'doctor') {
    const { checkDoctor, formatDoctor, warmDoctor } = await import('./lib/doctor.mjs');
    if (args.length > 2 || args.length === 2 && args[1] !== '--warm') {
      throw new TypeError('Use roster doctor [--warm].');
    }
    const result = checkDoctor();
    process.stdout.write(formatDoctor(result));
    if (!result.ok) process.exitCode = 1;
    if (args[1] === '--warm') await warmDoctor();
  } else if (args.length === 1 && args[0] === 'init') {
    const { formatInit, initializeRoster } = await import('./lib/init.mjs');
    process.stdout.write(formatInit(await initializeRoster()));
  } else if (args[0] === 'onboard' && args.length <= 2 && (args.length === 1 || args[1] === '--discover')) {
    const { runOnboard } = await import('./onboard/wizard.mjs');
    process.exitCode = (await runOnboard({ installationRoot: rosterRoot, discover: args[1] === '--discover' })).exitCode;
  } else if (args[0] === 'fleet') {
    const { runFleet } = await import('./lib/fleet-cli.mjs');
    const result = await runFleet(args.slice(1), { installationRoot: rosterRoot });
    if (result.exitCode !== undefined) process.exitCode = result.exitCode;
  } else if (args[0] === 'prepare') {
    const { runIssue } = await import('./lib/issue.mjs');
    if (args.length !== 3 || args[1] !== '--issue') {
      throw new TypeError('Use roster prepare --issue N.');
    }
    await runIssue(args[2]);
  } else if (args[0] === 'run' && args.includes('--ask-file')) {
    const { runDemo } = await import('./lib/demo.mjs');
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
    const { runBuiltinIssue, runBuiltinTask } = await import('./lib/builtin.mjs');
    const options = runOptions(args.slice(1));
    if (options.issue === undefined) await runBuiltinTask({ repoRoot: rosterRoot, debug });
    else {
      const result = await runBuiltinIssue(options.issue, { publish: options.publish, seats: options.seats,
        skipReview: options.skipReview,
        confirm: options.confirm,
        planMode: options.plan,
        autoModel: options.autoModel, repoRoot: rosterRoot, debug });
      if (result.failed) process.exitCode = 1;
    }
  } else if (args[0] === 'status') {
    const { loadConfig } = await import('./lib/config.mjs');
    const { formatStatus, readStatus } = await import('./lib/status.mjs');
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
    const { validateRecipe } = await import('./lib/recipe.mjs');
    validateRecipe(args[2]);
    process.stdout.write(`Valid recipe: ${args[2]}\n`);
  } else if (args.length === 3 && args[0] === 'vault' && args[1] === 'set') {
    await setVaultSecret(args[2]);
  } else if (args.length === 2 && args[0] === 'vault' && args[1] === 'list') {
    const { createFileVault } = await import('./vault/file.mjs');
    const names = await createFileVault().list();
    if (names.length) process.stdout.write(`${names.join('\n')}\n`);
  } else if (args.length === 3 && args[0] === 'vault' && args[1] === 'get') {
    await getVaultSecret(args[2]);
  } else if (args[0] === 'stats') {
    const { formatMetrics, loadMetrics, summarizeMetrics } = await import('./lib/metrics.mjs');
    const { deliveryMetrics, formatDeliveryMetrics, parseStatsOptions } = await import('./lib/delivery-metrics.mjs');
    const { resolveContractsPath } = await import('./lib/paths.mjs');
    const { repositoryRoot } = await import('./lib/learn.mjs');
    const options = parseStatsOptions(args.slice(1));
    const records = loadMetrics({
      ...options,
      evalsPath: options.evalsPath === undefined ? undefined : resolve(options.evalsPath),
      contractsPath: resolveContractsPath(),
      cwd: repositoryRoot(),
    });
    const groups = options.delivery ? deliveryMetrics(records) : summarizeMetrics(records);
    process.stdout.write(options.json ? `${JSON.stringify(groups, null, 2)}\n`
      : options.delivery ? formatDeliveryMetrics(groups) : formatMetrics(groups));
  } else if (args[0] === 'eval') {
    const { parseEvaluationArgs, recordEvaluation } = await import('./lib/eval.mjs');
    const { values, options } = parseEvaluationArgs(args.slice(1));
    const evaluation = await recordEvaluation(...values, options);
    process.stdout.write(`Recorded AI-Eval for ${evaluation.sha ?? evaluation.session}.\n`);
  } else if (args[0] === 'recommend') {
    const { parseRecommendationArgs } = await import('./lib/learn.mjs');
    const { loadAvailableMetrics } = await import('./lib/metrics.mjs');
    const { loadConfig } = await import('./lib/config.mjs');
    const { formatRoute, routeTask } = await import('./lib/route.mjs');
    const { resolveProjectRoot } = await import('./lib/paths.mjs');
    const { taskClass, difficulty } = parseRecommendationArgs(args.slice(1));
    const cwd = resolveProjectRoot();
    const route = await routeTask({ cwd, installationRoot: rosterRoot, taskClass,
      difficulty: difficulty ?? 2, records: loadAvailableMetrics({ cwd }) });
    process.stdout.write(formatRoute(route, taskClass, loadConfig({ cwd })));
  } else if (args[0] === 'history') {
    const { runHistory } = await import('./lib/history-cli.mjs');
    process.stdout.write(await runHistory(args.slice(1)));
  } else if (args[0] === 'clean') {
    const options = cleanOptions(args.slice(1));
    const clean = await import('./lib/clean-ops.mjs');
    const { resolveProjectRoot } = await import('./lib/paths.mjs');
    const projectRoot = options.stateRoot === undefined ? resolveProjectRoot() : undefined;
    const confirm = { execute: options.execute, yes: options.yes, interactive: Boolean(process.stdout.isTTY) };
    if (options.target === 'machine-history' || options.target === 'curated-memory') {
      const { resolveHistoryRoot } = await import('./lib/history-cli.mjs');
      const storeRoot = await resolveHistoryRoot({ cwd: projectRoot, storePath: options.storePath });
      const report = await clean.pruneProvenance(storeRoot, { ...confirm, target: options.target, repoRoot: projectRoot });
      process.stdout.write(clean.formatProvenanceReport(report));
    } else {
      const plan = options.target
        ? await clean.resolveCleanTarget(options.target, { repoRoot: projectRoot, issue: options.issue })
        : { handle: await clean.resolveStateRoot(projectRoot, {
          scope: options.scope ?? (options.stateRoot === undefined ? 'repo' : 'worktree'),
          stateRoot: options.stateRoot,
        }), exclude: [] };
      const report = await clean.runClean(plan.handle, { ...confirm, exclude: [...plan.exclude, ...options.exclude] });
      process.stdout.write(clean.formatCleanReport(report));
    }
  } else if (args.length === 1 && args[0] === 'bench') {
    const { runBench } = await import('./lib/bench.mjs');
    const result = await runBench({
      cwd: process.cwd(), repoRoot: rosterRoot,
      commandDispatchMs: performance.now() - cliStartedAt,
    });
    process.stdout.write(`${JSON.stringify(result.report, null, 2)}\n`);
  } else {
    throw new TypeError('Unknown arguments. Run roster --help for usage.');
  }

  function cleanOptions(args) {
    const options = {
      execute: false,
      yes: false,
      exclude: [],
    };
    const seen = new Set();
    const usage = 'Use roster clean [--target issue|repo|machine-history|curated-memory] [--issue N] [--store DIR] ' +
      '[--scope SCOPE] [--state-root PATH] [--exclude NAME]... [--execute --yes].';
    for (let index = 0; index < args.length; index += 1) {
      const flag = args[index];
      if (seen.has(flag) && flag !== '--exclude') {
        throw new TypeError(usage);
      }
      seen.add(flag);
      if (flag === '--execute') options.execute = true;
      else if (flag === '--yes') options.yes = true;
      else {
        const value = args[++index];
        if (!value || value.startsWith('--')) throw new TypeError(usage);
        if (flag === '--scope') options.scope = value;
        else if (flag === '--state-root') options.stateRoot = value;
        else if (flag === '--exclude') options.exclude.push(value);
        else if (flag === '--target') options.target = value;
        else if (flag === '--issue') options.issue = value;
        else if (flag === '--store') options.storePath = value;
        else throw new TypeError(usage);
      }
    }
    const provenance = options.target === 'machine-history' || options.target === 'curated-memory';
    if ((options.target && (options.scope || options.stateRoot)) ||
        (options.issue !== undefined && options.target !== 'issue') ||
        (options.storePath !== undefined && !provenance) ||
        (provenance && options.exclude.length)) {
      throw new TypeError(usage);
    }
    return options;
  }

  function runOptions(args) {
    const options = {};
    const seen = new Set();
    const usage = 'Use roster run --issue N [--runtime builtin] [--seats planner,coder,reviewer] ' +
      '[--parallel K] [--saved] [--publish] [--skip-review] [--confirm] [--plan], ' +
      'or roster run --seat coder --runtime builtin for an existing TASK.md.';
    for (let index = 0; index < args.length; index += 1) {
      const flag = args[index];
      if (!['--issue', '--seat', '--seats', '--runtime', '--parallel', '--auto-model', '--saved', '--publish', '--skip-review', '--confirm', '--plan'].includes(flag) ||
          seen.has(flag)) {
        throw new TypeError(usage);
      }
      seen.add(flag);
      if (flag === '--publish') options.publish = true;
      else if (flag === '--auto-model') options.autoModel = true;
      else if (flag === '--saved') options.saved = true;
      else if (flag === '--skip-review') options.skipReview = true;
      else if (flag === '--confirm') options.confirm = true;
      else if (flag === '--plan') options.plan = true;
      else {
        const value = args[++index];
        if (!value || value.startsWith('--')) throw new TypeError(usage);
        if (flag === '--parallel') {
          if (!/^[1-9]\d*$/.test(value) || !Number.isSafeInteger(Number(value))) throw new TypeError(usage);
          options.parallel = Number(value);
        } else options[flag.slice(2)] = value;
      }
    }
    if (options.issue === undefined && options.seat === 'coder' && options.runtime === 'builtin' &&
        options.seats === undefined && options.parallel === undefined && !options.autoModel && !options.publish && !options.skipReview && !options.confirm && !options.plan) return options;
    if (!options.issue || (options.runtime !== undefined && options.runtime !== 'builtin') ||
        (options.seat !== undefined && options.seat !== 'coder') ||
        (options.seats !== undefined &&
          !['planner,coder', 'planner,coder,reviewer'].includes(options.seats)) ||
        (options.seat !== undefined && options.seats !== undefined) ||
        options.parallel > 1 && (options.confirm || options.plan)) {
      throw new TypeError(usage);
    }
    return { ...options, autoModel: options.saved ? false : options.autoModel !== false, seats: 'planner,coder,reviewer' };
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
