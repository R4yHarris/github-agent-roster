// Spec §4.7, §5.5: a new acceptance test must fail against the base revision's product files and pass on the candidate.
import { execFile } from 'node:child_process';
import { existsSync, promises as fs } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { throwIfCancelled } from './cancel.mjs';
import { fullTestPerTestTimeoutMs, mirrorInitializedSubmodules } from './tools.mjs';

const execute = promisify(execFile);
const isTestFile = (file) => /(^|\/)[^/]+\.test\.[cm]?js$/.test(file);
const isProductCode = (file) => /\.[cm]?[jt]sx?$/.test(file) && !isTestFile(file) && !file.startsWith('tests/');
const unquote = (text) => text.replace(/\\(.)/g, '$1');

// Literal names passed to test()/it(); template or computed names cannot be matched against base and are skipped.
export function testNames(text) {
  const names = [];
  for (const match of String(text ?? '').matchAll(/\b(?:test|it)(?:\.only)?\(\s*(['"])((?:\\.|(?!\1)[^\\\n])*)\1/g)) {
    names.push(unquote(match[2]));
  }
  return names;
}

export function newTestNames(candidateText, baseText) {
  const base = new Set(baseText === null ? [] : testNames(baseText));
  return [...new Set(testNames(candidateText))].filter((name) => !base.has(name));
}

// Maps each TAP result line's test name to whether it passed; skipped and todo tests count as not passing.
export function tapResults(stdout) {
  const results = new Map();
  for (const match of String(stdout ?? '').matchAll(/^\s*(not ok|ok) \d+ - (.*?)(?: # (SKIP|TODO)\b.*)?$/gm)) {
    const name = match[2].replace(/\\#/g, '#').replace(/\\\\/g, '\\');
    results.set(name, match[1] === 'ok' && !match[3]);
  }
  return results;
}

export function notRedReason(notRed) {
  return `Not red: ${notRed.map(({ file, name }) => `"${name}" (${file})`).join(', ')} passed against the base ` +
    'revision\'s product files, so it does not prove this change. Make each new test assert behavior the Ask adds.';
}

export function redGreenTable(redGreen) {
  if (redGreen.status !== 'checked') return `Red/green: ${redGreen.status}${redGreen.reason ? ` (${redGreen.reason})` : ''}\n`;
  const cell = (text) => String(text).replaceAll('|', '\\|');
  return '| Test file | Test | Base | Candidate |\n|---|---|---|---|\n' + redGreen.tests.map(({ file, name, base, candidate }) =>
    `| ${cell(file)} | ${cell(name)} | ${base} | ${candidate} |`).join('\n') + '\n';
}

/**
 * Runs only the changed test files: once in a temporary base worktree overlaid with the candidate's changed
 * test files (old src, new tests), and once in the worktree. The worktree itself is never written.
 */
export async function checkRedGreen({ worktree, files, mode = 'required', env = {}, runCommand = execute, signal }) {
  if (mode !== 'required') return { status: mode === 'none' ? 'skipped' : 'exempt', tests: [], notRed: [] };
  const changed = files.map((file) => file.replaceAll('\\', '/'));
  // A test-only change has no product behavior for a new test to be red against.
  if (!changed.some(isProductCode)) return { status: 'exempt', reason: 'test-only change', tests: [], notRed: [] };
  const candidates = [];
  for (const file of changed.filter(isTestFile)) {
    const text = await fs.readFile(path.join(worktree, ...file.split('/')), 'utf8').catch(() => null);
    if (text === null) continue;
    let baseText = null;
    try {
      ({ stdout: baseText } = await runCommand('git', ['show', `HEAD:${file}`], {
        cwd: worktree, timeout: 30_000, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024, signal,
      }));
    } catch (error) {
      throwIfCancelled(signal);
      if (!/exists on disk, but not in|does not exist in|fatal: path/.test(String(error.stderr ?? error.message))) {
        return { status: 'unavailable', reason: 'base revision unreadable', tests: [], notRed: [] };
      }
    }
    const names = newTestNames(text, baseText);
    if (names.length) candidates.push({ file, names });
  }
  if (!candidates.length) return { status: 'none', tests: [], notRed: [] };
  const testEnv = { ...env, ROSTER_SEAT: 'coder' };
  for (const name of ['GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH', 'GH_TOKEN', 'GITHUB_TOKEN', 'NODE_TEST_CONTEXT']) {
    delete testEnv[name];
  }
  const runFile = async (cwd, file, runEnv) => {
    try {
      return (await runCommand(process.execPath, ['--test', '--test-reporter=tap',
        `--test-timeout=${fullTestPerTestTimeoutMs}`, file], {
        cwd, timeout: fullTestPerTestTimeoutMs * 2, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024, env: runEnv, signal,
      })).stdout;
    } catch (error) {
      throwIfCancelled(signal);
      return String(error.stdout ?? '');
    }
  };
  const temp = await fs.mkdtemp(path.join(os.tmpdir(), 'roster-red-'));
  const base = path.join(temp, 'base');
  const tests = [];
  try {
    try {
      await runCommand('git', ['worktree', 'add', '--detach', base, 'HEAD'], {
        cwd: worktree, timeout: 120_000, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024, signal,
      });
      await mirrorInitializedSubmodules(worktree, base);
      // Old product files, new tests: overlay every changed file under tests/ (helpers and fixtures too).
      for (const file of changed.filter((entry) => entry.startsWith('tests/') || isTestFile(entry))) {
        const from = path.join(worktree, ...file.split('/'));
        if (!existsSync(from)) continue;
        const to = path.join(base, ...file.split('/'));
        await fs.mkdir(path.dirname(to), { recursive: true });
        await fs.copyFile(from, to);
      }
    } catch (error) {
      throwIfCancelled(signal);
      return { status: 'unavailable', reason: String(error.message ?? error).split('\n')[0].slice(0, 200), tests: [], notRed: [] };
    }
    const vendor = path.join(worktree, 'vendor', 'github-agent-contracts');
    const baseEnv = { ...testEnv, ...(existsSync(path.join(vendor, 'scripts', 'agent-pr.mjs'))
      ? { GITHUB_AGENT_CONTRACTS: vendor } : {}) };
    for (const { file, names } of candidates) {
      const atBase = tapResults(await runFile(base, file, baseEnv));
      const atCandidate = tapResults(await runFile(worktree, file, testEnv));
      for (const name of names) {
        tests.push({ file, name, base: atBase.get(name) ? 'pass' : 'fail',
          candidate: atCandidate.has(name) ? atCandidate.get(name) ? 'pass' : 'fail' : 'not run' });
      }
    }
  } finally {
    await runCommand('git', ['worktree', 'remove', '--force', base], {
      cwd: worktree, timeout: 120_000, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024,
    }).catch(() => {});
    await fs.rm(temp, { recursive: true, force: true, maxRetries: 3 }).catch(() => {});
  }
  return { status: 'checked', tests, notRed: tests.filter(({ base: result }) => result === 'pass') };
}
