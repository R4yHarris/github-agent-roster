// Spec §4.2, §5.5: before coding, confirm deterministically which acceptance checks already hold.
import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { throwIfCancelled } from './cancel.mjs';
import { fullTestPerTestTimeoutMs } from './tools.mjs';

const execute = promisify(execFile);
const testCommand = /`node --test((?:\s+[^`\s]+\.test\.[cm]?js)+)\s*`/;
const commandOnly = /^`node --test[^`]*`\s+(?:exits|exit with|returns)\s+(?:code\s+)?0\.?$/i;
const exportReference = /`([A-Za-z_$][\w$]*)`\s+(?:is exported\s+)?(?:from|in)\s+`([^`]+\.[cm]?js)`/g;
const secretEnv = ['GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH', 'GH_TOKEN', 'GITHUB_TOKEN', 'NODE_TEST_CONTEXT'];
export const isTestPath = (file) => /(^|\/)[^/]+\.test\.[cm]?js$/.test(file) || file.startsWith('tests/');

function exportsName(text, name) {
  const escaped = name.replace(/\$/g, '\\$');
  if (new RegExp(`\\bexport\\s+(?:default\\s+)?(?:async\\s+)?(?:function\\*?|const|let|var|class)\\s+${escaped}\\b`).test(text)) return true;
  for (const [, names] of text.matchAll(/\bexport\s*\{([^}]*)\}/g)) {
    if (names.split(',').some((entry) => entry.trim().split(/\s+as\s+/).pop() === name)) return true;
  }
  return false;
}

async function readFile(worktree, file) {
  const target = path.resolve(worktree, file);
  if (path.relative(worktree, target).startsWith('..')) return null;
  return fs.readFile(target, 'utf8').catch(() => null);
}

/**
 * Classifies each acceptance check as met, unmet or unknown with evidence, using only the worktree: named test
 * files are run with the injected runner, and backticked exports are looked up in their named modules. Prose
 * stays unknown. No model or network call is made.
 */
export async function triageChecks({ worktree, checks = [], runCommand = execute, env = {}, signal }) {
  const root = path.resolve(worktree);
  const testEnv = { ...env, ROSTER_SEAT: 'coder' };
  for (const name of secretEnv) delete testEnv[name];
  const runs = new Map();
  const run = async (files) => {
    const key = files.join(' ');
    if (!runs.has(key)) {
      runs.set(key, runCommand(process.execPath, ['--test', '--test-reporter=tap',
        `--test-timeout=${fullTestPerTestTimeoutMs}`, ...files], {
        cwd: root, timeout: fullTestPerTestTimeoutMs * 2, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024, env: testEnv, signal,
      }).then(() => true, () => {
        throwIfCancelled(signal);
        return false;
      }));
    }
    return runs.get(key);
  };
  const entries = [];
  for (const check of checks.map((entry) => String(entry).trim()).filter(Boolean)) {
    const evidence = [];
    let unmet = false;
    for (const [, name, file] of check.matchAll(exportReference)) {
      const text = await readFile(root, file);
      if (text === null) {
        unmet = true;
        evidence.push(`${file} does not exist`);
      } else if (exportsName(text, name)) {
        evidence.push(`\`${name}\` is already exported from ${file}`);
      } else {
        unmet = true;
        evidence.push(`\`${name}\` is not exported from ${file}`);
      }
    }
    let met = false;
    const command = check.match(testCommand);
    if (command) {
      const files = command[1].trim().split(/\s+/);
      const absent = [];
      for (const file of files) if (await readFile(root, file) === null) absent.push(file);
      if (absent.length) {
        unmet = true;
        evidence.push(`${absent.join(', ')} does not exist yet`);
      } else if (await run(files)) {
        evidence.push(`\`node --test ${files.join(' ')}\` already passes`);
        met = commandOnly.test(check);
      } else {
        unmet = true;
        evidence.push(`\`node --test ${files.join(' ')}\` fails now`);
      }
    }
    entries.push({ check, status: unmet ? 'unmet' : met ? 'met' : 'unknown', evidence });
  }
  return { entries, allMet: entries.length > 0 && entries.every(({ status }) => status === 'met') };
}

export function triageText({ entries, allMet }, { limit = 1500 } = {}) {
  if (!entries.some(({ status, evidence }) => status !== 'unknown' || evidence.length)) return '';
  const lead = allMet
    ? 'Every acceptance check already holds before any edit. This slice is tests-only: pin the behavior with tests ' +
      'and do not change product code.'
    : 'Met checks need a pinning test, not new product code. Unknown checks were not verified here; read the code ' +
      'that would satisfy them before writing anything new.';
  const lines = entries.map(({ check, status, evidence }) => {
    const label = check.length > 120 ? `${check.slice(0, 117)}...` : check;
    return `- ${status}: ${label}${evidence.length ? ` (${evidence.join('; ')})` : ''}`;
  });
  const text = `${lead}\n\n${lines.join('\n')}`;
  return text.length > limit ? `${text.slice(0, limit - 3)}...` : text;
}
