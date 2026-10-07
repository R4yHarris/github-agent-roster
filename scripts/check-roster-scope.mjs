import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const execute = promisify(execFile);

const rosterRoot = fileURLToPath(new URL('../', import.meta.url));

/**
 * Commit-scope guard for runtime state (issue #232).
 *
 * The repo `.gitignore` covers every runtime state path, but stale or forced
 * `git add`s can still leave runtime state in the index. This check fails
 * when any managed runtime state path is tracked or staged, so a commit
 * cannot accidentally ship secrets or local state.
 */

// Every runtime state subtree the repo gitignore must cover. Tracked
// templates and docs are deliberately NOT listed here: only runtime state
// is protected from commits.
export const RUNTIME_STATE_PATTERNS = Object.freeze([
  '.roster/runs/',
  '.roster/logs/',
  '.roster/history',
  '.roster/history.*',
  '.roster/checkpoints/',
  '.roster/map.md',
  '.roster/evals.jsonl',
  '.roster/config.yml',
  '.roster/config.yml.*',
  '.roster/fleet.yml',
  '.roster/fleet.yml.*',
  '.roster/capabilities.yml',
  '.roster/capabilities.yml.*',
  '.roster/memory/',
  '.roster/asks/',
  '.roster-state/',
  '/evals.jsonl',
]);

export class RosterScopeError extends Error {
  constructor(message, { paths = [] } = {}) {
    super(message);
    this.name = 'RosterScopeError';
    this.code = 'E_ROSTER_SCOPE';
    this.paths = paths;
  }
}

function git(repoRoot, ...args) {
  return execute('git', ['--no-pager', ...args], {
    cwd: repoRoot, encoding: 'utf8', timeout: 15_000,
  });
}

/**
 * Verify that `.gitignore` covers every runtime state pattern.
 * Fails when a pattern is missing from the tracked `.gitignore`.
 */
export async function checkGitignoreCoverage({ repoRoot = rosterRoot } = {}) {
  const file = join(repoRoot, '.gitignore');
  let source;
  try {
    source = await fs.readFile(file, 'utf8');
  } catch (error) {
    throw new RosterScopeError(
      `missing .gitignore at ${file}; runtime state paths must be ignored before first write.`,
    );
  }
  const lines = new Set(source.replace(/\r\n/g, '\n').split('\n'));
  const missing = RUNTIME_STATE_PATTERNS.filter((pattern) => {
    if (lines.has(pattern)) return false;
    // A blanket parent-directory rule also covers the pattern
    // (e.g. `.roster/` covers `.roster/config.yml`).
    const parent = dirname(pattern).split(sep).join('/');
    return parent === '.' || !lines.has(`${parent.replace(/\/$/, '')}/`);
  });
  if (missing.length) {
    throw new RosterScopeError(
      `.gitignore does not cover runtime state paths: ${missing.join(', ')}. ` +
      'Add them so runtime state is never committed.',
      { paths: missing },
    );
  }
  return { ok: true, patterns: RUNTIME_STATE_PATTERNS };
}

/**
 * Fail when any runtime state path is tracked in the index or staged for
 * commit. Returns the offending paths when found.
 */
export async function checkStagedRuntimeState({ repoRoot = rosterRoot } = {}) {
  // Files already tracked, plus any staged path (ls-files covers the index,
  // which is exactly what "staged" means for commit purposes).
  const { stdout: tracked } = await git(repoRoot, 'ls-files', '-z');
  const candidates = [...new Set(tracked.split('\0').filter(Boolean))];
  const offenders = candidates.filter((file) => {
    const normalized = file.split('\\').join('/');
    return RUNTIME_STATE_PATTERNS.some((pattern) => {
      if (pattern.endsWith('/')) return normalized.startsWith(pattern);
      if (pattern.includes('*')) {
        const base = pattern.split('*')[0];
        return normalized === pattern || normalized.startsWith(base);
      }
      return normalized === pattern;
    });
  });
  if (offenders.length) {
    throw new RosterScopeError(
      `runtime state must never be committed; untrack: ${offenders.join(', ')}. ` +
      'Remove it from the index with `git rm --cached` and keep the .gitignore rules.',
      { paths: offenders },
    );
  }
  return { ok: true, checked: candidates.length };
}

/**
 * Full commit-scope check: ignore rules present AND no runtime state staged.
 */
export async function checkRosterScope(options = {}) {
  const ignore = await checkGitignoreCoverage(options);
  const staged = await checkStagedRuntimeState(options);
  return { ...ignore, ...staged };
}

// Run directly: `node scripts/check-roster-scope.mjs [repoRoot]`.
const isMain = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  const repoRoot = process.argv[2] ? resolve(process.argv[2]) : rosterRoot;
  try {
    const result = await checkRosterScope({ repoRoot });
    console.log(`roster scope ok: ${result.patterns.length} ignore patterns, ${result.checked} indexed files checked.`);
  } catch (error) {
    console.error(`roster scope check failed: ${error.message}`);
    process.exit(1);
  }
}
