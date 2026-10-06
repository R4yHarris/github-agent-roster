// `npm test`: run every tests/*.test.mjs file in its own `node --test` process from a longest-first pool.
// Node sorts the files it is given, so the pool (not argument order) makes the slowest files start first.
// Measured wall times persist in the Git common dir, shared by worktrees and never part of a diff.
import { execFileSync, spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const defaultFileBudgetMs = 60_000;
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const summaryReporter = pathToFileURL(path.join(root, 'scripts', 'summary-reporter.mjs')).href;

export function testJobs(env = process.env, available = os.availableParallelism()) {
  const requested = Number.parseInt(env.ROSTER_TEST_JOBS ?? '', 10);
  return Number.isInteger(requested) && requested > 0 ? requested : Math.max(1, available - 1);
}

export function fileBudgetMs(env = process.env) {
  const requested = Number.parseInt(env.ROSTER_TEST_FILE_BUDGET_MS ?? '', 10);
  return Number.isInteger(requested) && requested > 0 ? requested : defaultFileBudgetMs;
}

// Unmeasured files first (they may be slow), then measured files slowest first.
export function orderTestFiles(files, timings = {}) {
  const known = (file) => Number.isFinite(timings[file]);
  return [...files].sort((left, right) => {
    if (known(left) !== known(right)) return known(left) ? 1 : -1;
    if (!known(left)) return left.localeCompare(right);
    return timings[right] - timings[left] || left.localeCompare(right);
  });
}

export function overBudget(timings, budgetMs) {
  return Object.entries(timings).filter(([, ms]) => ms > budgetMs).sort((left, right) => right[1] - left[1]);
}

// New measurements replace old ones; entries for deleted test files are dropped.
export function mergeTimings(previous, measured, exists = (file) => existsSync(path.resolve(root, file))) {
  return Object.fromEntries(Object.entries({ ...previous, ...measured })
    .filter(([file, ms]) => Number.isFinite(ms) && exists(file)).sort(([left], [right]) => left.localeCompare(right)));
}

export function listTestFiles(directory = path.join(root, 'tests')) {
  return readdirSync(directory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith('.test.mjs'))
    .map((entry) => `tests/${entry.name}`).sort();
}

function timingsPath() {
  try {
    const common = execFileSync('git', ['rev-parse', '--git-common-dir'],
      { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
    return path.join(path.resolve(root, common), 'roster-test-timings.json');
  } catch {
    return path.join(os.tmpdir(), 'roster-test-timings.json');
  }
}

function readTimings(file) {
  try {
    const parsed = JSON.parse(readFileSync(file, 'utf8'));
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

function runFile(file, destination, children) {
  const started = Date.now();
  const env = { ...process.env };
  // An inherited test context makes the child skip its files (e.g. when this runner is itself under test).
  delete env.NODE_TEST_CONTEXT;
  return new Promise((resolve) => {
    const child = spawn(process.execPath, ['--test', '--test-reporter=spec', '--test-reporter-destination=stdout',
      `--test-reporter=${summaryReporter}`, `--test-reporter-destination=${destination}`, file],
    { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
    children.add(child);
    let output = '';
    child.stdout.on('data', (chunk) => { output += chunk; });
    child.stderr.on('data', (chunk) => { output += chunk; });
    child.on('close', (code, signal) => {
      children.delete(child);
      let summary = null;
      try { summary = JSON.parse(readFileSync(destination, 'utf8')); } catch { /* crashed before reporting */ }
      resolve({ file, code: code ?? 1, signal, ms: Date.now() - started, output, summary });
    });
  });
}

export async function runTests({ files, jobs, budgetMs, verbose = false, cache = timingsPath(),
  write = (text) => process.stdout.write(text) }) {
  const previous = readTimings(cache);
  const queue = orderTestFiles(files, previous);
  const scratch = mkdtempSync(path.join(os.tmpdir(), 'roster-tests-'));
  const children = new Set();
  const interrupt = () => {
    for (const child of children) child.kill();
    rmSync(scratch, { recursive: true, force: true });
    process.exit(130);
  };
  process.once('SIGINT', interrupt);
  const results = [];
  const started = Date.now();
  let next = 0;
  write(`Running ${queue.length} test files with ${Math.min(jobs, queue.length)} workers, slowest first.\n`);
  const worker = async () => {
    while (next < queue.length) {
      const index = next++;
      const result = await runFile(queue[index], path.join(scratch, `${index}.json`), children);
      result.failed = result.code !== 0 || Boolean(result.signal) || !result.summary;
      results.push(result);
      const counts = result.summary ? `${result.summary.pass}/${result.summary.tests} passed` : 'no summary';
      write(`${result.failed ? '✖' : '✔'} ${result.file} (${counts}, ${(result.ms / 1000).toFixed(1)}s)\n`);
      if (result.failed || verbose) write(`${result.output.trimEnd()}\n`);
    }
  };
  try {
    await Promise.all(Array.from({ length: Math.min(jobs, queue.length) }, worker));
  } finally {
    process.off('SIGINT', interrupt);
    rmSync(scratch, { recursive: true, force: true });
  }
  const measured = Object.fromEntries(results.filter((result) => result.summary).map((result) => [result.file, result.ms]));
  try {
    writeFileSync(cache, `${JSON.stringify(mergeTimings(previous, measured), null, 2)}\n`);
  } catch { /* timings only order the next run */ }

  const total = { tests: 0, pass: 0, fail: 0, skipped: 0, todo: 0 };
  for (const { summary } of results) for (const key of Object.keys(total)) total[key] += summary?.[key] ?? 0;
  const failedFiles = results.filter((result) => result.failed);
  write(`\nℹ files ${results.length}\nℹ tests ${total.tests}\nℹ pass ${total.pass}\nℹ fail ${total.fail}\n` +
    `ℹ skipped ${total.skipped}\nℹ todo ${total.todo}\nℹ wall ${((Date.now() - started) / 1000).toFixed(1)}s\n`);
  for (const result of failedFiles) {
    write(`✖ ${result.file}${result.summary?.failures.length ? `: ${result.summary.failures.join('; ')}` : ''}\n`);
  }
  const slow = overBudget(measured, budgetMs);
  if (slow.length) {
    write(`\n⚠ ${slow.length} test file(s) exceeded the ${budgetMs / 1000}s per-file budget; the slowest file bounds ` +
      'the wall time. Split each into tests/<module>.<topic>.test.mjs shards (see docs/TESTING.md):\n');
    for (const [file, ms] of slow) write(`  ${file} ${(ms / 1000).toFixed(1)}s\n`);
  }
  return failedFiles.length ? 1 : 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const requested = process.argv.slice(2).map((file) => file.replaceAll('\\', '/'));
  process.exitCode = await runTests({
    files: requested.length ? requested : listTestFiles(), jobs: testJobs(), budgetMs: fileBudgetMs(),
    verbose: process.env.ROSTER_TEST_VERBOSE === '1',
  });
}
