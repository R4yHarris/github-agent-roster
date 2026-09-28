#!/usr/bin/env node

import { runIssue } from './lib/issue.mjs';
import { formatMetrics, loadMetrics, summarizeMetrics } from './lib/metrics.mjs';
import { resolveContractsPath } from './lib/paths.mjs';
import { validateRecipe } from './lib/recipe.mjs';

const help = `Usage:
  roster --help
  roster run --issue N
  roster recipe validate PATH
  roster stats [--ref REVISION_OR_RANGE] [--evals PATH]

Run assigns one GitHub issue to a coder worktree. Recipe validates strict v0
seat YAML. Stats reads local AI-Run history from the sibling contracts pack.
`;

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
  if (args.length === 0 || (args.length === 1 && ['--help', '-h'].includes(args[0]))) {
    process.stdout.write(help);
  } else if (args.length === 3 && args[0] === 'run' && args[1] === '--issue') {
    await runIssue(args[2]);
  } else if (args.length === 3 && args[0] === 'recipe' && args[1] === 'validate') {
    validateRecipe(args[2]);
    process.stdout.write(`Valid recipe: ${args[2]}\n`);
  } else if (args[0] === 'stats') {
    const options = statsOptions(args.slice(1));
    const records = loadMetrics({ ...options, contractsPath: resolveContractsPath() });
    process.stdout.write(formatMetrics(summarizeMetrics(records)));
  } else {
    throw new TypeError('Unknown arguments. Run roster --help for usage.');
  }
}

try {
  await main(process.argv.slice(2));
} catch (error) {
  if (!(error instanceof Error)) throw error;
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
