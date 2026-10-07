import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

import {
  STATE_SCOPES,
  ContainmentError,
  canonicalPath,
  ensureContainedPath,
  isContainedIn,
  normalizeScope,
  resolveStateRoot as resolveStateRootViaPaths,
} from './paths.mjs';
import { resolveRepoIdentity } from './repo-identity.mjs';
import { RepoLockError, acquireRepoLock } from './repo-locks.mjs';

/**
 * Safe path resolution and state-scope validation for clean operations.
 *
 * Every state root is resolved through `src/lib/paths.mjs`, and every path
 * that leaves this module is verified to stay inside that state root. Root,
 * home and repository directories are never resolved as state roots and are
 * never handed to a destructive operation.
 */

export class CleanOpsError extends Error {
  constructor(message, code, details) {
    super(message);
    this.name = 'CleanOpsError';
    this.code = code;
    if (details !== undefined) {
      this.details = details;
    }
  }
}

const ESCAPE_CODE = 'CLEAN_OPS_PATH_ESCAPE';
const FORBIDDEN_CODE = 'CLEAN_OPS_FORBIDDEN_PATH';
const SCOPE_CODE = 'CLEAN_OPS_SCOPE_MISMATCH';
const CONFIRMATION_CODE = 'CLEAN_OPS_CONFIRMATION_REQUIRED';
const LOCK_CODE = 'CLEAN_OPS_LOCK_HELD';
const LOCK_PROBE_CODE = 'CLEAN_OPS_LOCK_PROBE_FAILED';
const PARTIAL_CODE = 'CLEAN_OPS_PARTIAL_CLEAN';

function isPlainObject(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Normalize to a plain absolute string path via paths.mjs `canonicalPath`.
 * `canonicalPath` is asynchronous, so this helper awaits it and falls back to
 * `path.resolve` when the canonical form cannot be computed (for example for
 * paths that do not exist on disk yet).
 */
async function normalizePath(value) {
  if (typeof value !== 'string' || value.length === 0) {
    return null;
  }
  try {
    const canonical = await canonicalPath(value);
    if (typeof canonical === 'string' && canonical.length > 0) {
      return canonical;
    }
  } catch {
    // fall through to a plain resolve
  }
  return path.resolve(value);
}

function samePath(a, b) {
  if (typeof a !== 'string' || typeof b !== 'string') {
    return false;
  }
  const left = a.replace(/[/\\]+$/, '');
  const right = b.replace(/[/\\]+$/, '');
  if (left === right) {
    return true;
  }
  return left.toLowerCase() === right.toLowerCase();
}

/**
 * Directories that must never be treated as state roots or removed.
 */
export async function forbiddenStateRoots({ repoRoot, worktreeRoot } = {}) {
  const list = [];
  const push = async (label, value) => {
    const normalized = await normalizePath(value);
    if (normalized) {
      list.push({ label, path: normalized });
    }
  };

  await push('root', path.parse(process.cwd()).root);
  await push('home', os.homedir());
  await push('repo', repoRoot);
  await push('worktree', worktreeRoot);

  const seen = new Set();
  return list.filter((entry) => {
    const key = entry.path.toLowerCase();
    if (seen.has(key)) {
      return false;
    }
    seen.add(key);
    return true;
  });
}

async function assertNotForbidden(root, forbidden) {
  const target = await normalizePath(root);
  if (!target) {
    throw new CleanOpsError('a non-empty path is required', FORBIDDEN_CODE, { path: root });
  }
  for (const entry of forbidden) {
    if (samePath(entry.path, target)) {
      throw new CleanOpsError(
        `refusing to use protected directory as a state root: ${entry.label} (${entry.path})`,
        FORBIDDEN_CODE,
        { label: entry.label, path: entry.path }
      );
    }
  }
  return target;
}

function scopeOf(handle) {
  return typeof handle?.scope === 'string' ? handle.scope : undefined;
}

function rootOf(handle) {
  const root = handle?.root ?? handle?.stateRoot;
  return typeof root === 'string' && root.length > 0 ? root : undefined;
}

/**
 * Validate that a state handle carries the expected scope.
 */
export function assertStateScope(handle, expectedScope) {
  if (!isPlainObject(handle)) {
    throw new CleanOpsError('state root handle is required', SCOPE_CODE);
  }
  const actual = scopeOf(handle);
  if (typeof actual !== 'string' || actual.length === 0) {
    throw new CleanOpsError('state root handle is missing a scope', SCOPE_CODE, { expected: expectedScope });
  }
  if (expectedScope !== undefined && expectedScope !== null) {
    const wanted = normalizeScope(expectedScope);
    if (actual !== wanted) {
      throw new CleanOpsError(
        `state root scope mismatch: expected "${wanted}" but got "${actual}"`,
        SCOPE_CODE,
        { expected: wanted, actual }
      );
    }
  }
  return actual;
}

/**
 * Validate and normalize a scope option against paths.mjs STATE_SCOPES.
 */
function assertScopeOption(scope) {
  if (typeof scope !== 'string' || scope.length === 0) {
    throw new CleanOpsError('a state scope is required', SCOPE_CODE, { scope });
  }
  if (!STATE_SCOPES.includes(scope)) {
    throw new CleanOpsError(`unknown state scope "${scope}"`, SCOPE_CODE, { scope, known: STATE_SCOPES });
  }
  const normalized = normalizeScope(scope);
  if (typeof normalized !== 'string' || !STATE_SCOPES.includes(normalized)) {
    throw new CleanOpsError(`unknown state scope "${scope}"`, SCOPE_CODE, { scope, known: STATE_SCOPES });
  }
  return normalized;
}

/**
 * Resolve the repository identity for a repository root via
 * `src/lib/repo-identity.mjs`. `run` is the child-process seam documented by
 * repo-identity.mjs, so callers and tests can prove provenance without
 * shelling out to git.
 */
export async function resolveRepositoryId(repoRoot, { run } = {}) {
  if (typeof repoRoot !== 'string' || repoRoot.length === 0) {
    throw new CleanOpsError('a repository root is required to resolve repository identity', FORBIDDEN_CODE);
  }
  const identity = await resolveRepoIdentity({ repoRoot, run });
  if (typeof identity !== 'string' || identity.length === 0) {
    throw new CleanOpsError(
      `repository identity could not be resolved for ${repoRoot}`,
      FORBIDDEN_CODE,
      { repoRoot }
    );
  }
  return identity;
}

/**
 * Resolve a state root for a repository. Always delegates to paths.mjs.
 */
export async function resolveStateRoot(repoRoot, options = {}) {
  if (!isPlainObject(options)) {
    throw new CleanOpsError('options must be an object', SCOPE_CODE);
  }

  const scope = assertScopeOption(options.scope ?? 'repo');
  const forbidden = await forbiddenStateRoots({
    repoRoot,
    worktreeRoot: options.worktreeRoot,
  });

  const repoId =
    typeof repoRoot === 'string' && repoRoot.length > 0
      ? await resolveRepositoryId(repoRoot, { run: options.run })
      : null;

  if (options.stateRoot !== undefined) {
    const override = await assertNotForbidden(options.stateRoot, forbidden);
    await fsp.mkdir(override, { recursive: true });
    return {
      scope,
      root: override,
      repoId,
      worktreeId: null,
      runId: null,
      platform: os.platform(),
      home: os.homedir(),
      writable: true,
      resolvedRepoId: repoId,
    };
  }

  if (!repoId) {
    throw new CleanOpsError('a repository root is required to resolve a state root', FORBIDDEN_CODE);
  }

  // paths.mjs owns every state-root path; the repository identity that names
  // the state directory comes from src/lib/repo-identity.mjs and is injected
  // through paths.mjs' documented ROSTER_REPO_ID seam.
  const env = { ...(options.env ?? process.env), ROSTER_REPO_ID: repoId };
  const handle = await resolveStateRootViaPaths({
    repoRoot,
    machineRoot: options.machineRoot,
    env,
  });

  const root = rootOf(handle);
  if (!root) {
    throw new CleanOpsError('paths.resolveStateRoot did not return a state root', FORBIDDEN_CODE, { handle });
  }
  await assertNotForbidden(root, forbidden);
  assertStateScope(handle, scope);
  return { ...handle, repoId: handle.repoId ?? repoId, resolvedRepoId: repoId };
}

/**
 * Resolve `relativePath` inside the state root, refusing traversal,
 * symlink/reparse escapes and protected directories.
 */
export async function resolveStatePath(handle, relativePath = '.') {
  assertStateScope(handle);
  const root = rootOf(handle);
  if (!root) {
    throw new CleanOpsError('state root handle is missing a root', FORBIDDEN_CODE, { handle });
  }
  if (typeof relativePath !== 'string') {
    throw new CleanOpsError('relativePath must be a string', ESCAPE_CODE);
  }

  const canonicalRoot = await normalizePath(root);
  if (!canonicalRoot) {
    throw new CleanOpsError('state root handle is missing a root', FORBIDDEN_CODE, { handle });
  }
  const candidate = path.resolve(canonicalRoot, relativePath);
  // Anything outside the root is an escape, even when it also lands on a
  // protected directory such as `/` for `../..` from a shallow temp root.
  if (!isContainedIn(candidate, canonicalRoot)) {
    throw new CleanOpsError(
      `refusing to resolve path outside the state root: ${relativePath}`,
      ESCAPE_CODE,
      { root: canonicalRoot, candidate }
    );
  }

  const forbidden = await forbiddenStateRoots({
    repoRoot: handle.repoRoot,
    worktreeRoot: handle.worktreeRoot,
  });
  forbidden.push({ label: 'state-root', path: canonicalRoot });

  await assertNotForbidden(candidate, forbidden);

  let resolved;
  let viaPaths;
  try {
    resolved = await ensureContainedPath(canonicalRoot, candidate);
    viaPaths = true;
  } catch (error) {
    if (error instanceof CleanOpsError) {
      throw error;
    }
    if (error instanceof ContainmentError) {
      // paths.mjs rejected a traversal or symlink/reparse-point escape.
      throw new CleanOpsError(error.message, ESCAPE_CODE, {
        root: canonicalRoot,
        candidate,
        cause: error,
        // Provenance marker: this rejection came from paths.mjs' containment
        // walk, not from the local realpath fallback.
        via: 'paths.mjs',
      });
    }
    // Fall back to a manual containment + realpath check when paths.mjs has no
    // opinion about this shape.
    resolved = await manualContain(canonicalRoot, candidate);
  }

  const resolvedTarget = typeof resolved === 'string' ? resolved : resolved?.target;
  const canonical = (await normalizePath(resolvedTarget ?? candidate)) ?? candidate;
  if (!isContainedIn(canonical, canonicalRoot)) {
    throw new CleanOpsError(
      `refusing to resolve path outside the state root: ${relativePath}`,
      ESCAPE_CODE,
      { root: canonicalRoot, candidate }
    );
  }
  return canonical;
}

async function manualContain(canonicalRoot, candidate) {
  const real = await fsp.realpath(candidate).catch(() => null);
  const target = real ? await normalizePath(real) : await normalizePath(candidate);
  if (!target || !canonicalRoot || !isContainedIn(target, canonicalRoot)) {
    const error = new CleanOpsError(
      `refusing to resolve path outside the state root: ${candidate}`,
      ESCAPE_CODE,
      { root: canonicalRoot, candidate }
    );
    error.name = 'ContainmentError';
    throw error;
  }
  return candidate;
}

/**
 * Detect whether `targetPath` escapes the state root via a symlink or reparse
 * point, or resolves to a protected directory.
 */
export async function assertSafeStatePath(handle, targetPath) {
  assertStateScope(handle);
  const root = rootOf(handle);
  if (!root) {
    throw new CleanOpsError('state root handle is missing a root', FORBIDDEN_CODE, { handle });
  }
  if (typeof targetPath !== 'string') {
    throw new CleanOpsError('relativePath must be a string', ESCAPE_CODE, { handle });
  }

  const canonicalRoot = await normalizePath(root);
  const candidate = path.resolve(canonicalRoot ?? root, targetPath);
  if (!canonicalRoot || !isContainedIn(candidate, canonicalRoot)) {
    throw new CleanOpsError(
      `refusing to resolve path outside the state root: ${targetPath}`,
      ESCAPE_CODE,
      { root: canonicalRoot ?? root, candidate }
    );
  }

  // Reject symlink/reparse-point entries before any deeper resolution, so a
  // linked entry can never be followed out of the state root.
  const stat = await fsp.lstat(candidate).catch(() => null);
  if (stat && stat.isSymbolicLink()) {
    throw new CleanOpsError(
      `refusing to operate on a symlink or reparse point inside the state root: ${candidate}`,
      ESCAPE_CODE,
      { root: canonicalRoot, target: candidate }
    );
  }

  return resolveStatePath(handle, targetPath);
}

async function removeRecursively(target) {
  await fsp.rm(target, { recursive: true, force: true, maxRetries: 2 });
}

/**
 * Delete a path inside a state root after validating containment, scope and
 * that the target is not a root, home, repository or state-root directory.
 */
export async function removeStatePath(handle, relativePath, options = {}) {
  assertStateScope(handle, options.scope);
  const root = rootOf(handle);
  if (!root) {
    throw new CleanOpsError('state root handle is missing a root', FORBIDDEN_CODE, { handle });
  }

  const forbidden = await forbiddenStateRoots({
    repoRoot: handle.repoRoot,
    worktreeRoot: handle.worktreeRoot,
  });
  const canonicalRoot = await normalizePath(root);
  forbidden.push({ label: 'state-root', path: canonicalRoot });

  const target = await assertSafeStatePath(handle, relativePath);
  const normalizedTarget = await normalizePath(target);
  await assertNotForbidden(normalizedTarget, forbidden);

  if (!isContainedIn(normalizedTarget, canonicalRoot)) {
    throw new CleanOpsError(
      `refusing to resolve path outside the state root: ${relativePath}`,
      ESCAPE_CODE,
      { root: canonicalRoot, target }
    );
  }

  if (options.dryRun) {
    return { removed: false, target: normalizedTarget, dryRun: true };
  }
  await removeRecursively(normalizedTarget);
  return { removed: true, target: normalizedTarget, dryRun: false };
}

/**
 * Ensure a state directory exists for `handle` without creating anything
 * outside the resolved state root.
 */
export async function ensureStateDirectory(handle) {
  assertStateScope(handle);
  const root = rootOf(handle);
  if (!root) {
    throw new CleanOpsError('state root handle is missing a root', FORBIDDEN_CODE, { handle });
  }
  const safe = await assertNotForbidden(
    root,
    await forbiddenStateRoots({ repoRoot: handle.repoRoot, worktreeRoot: handle.worktreeRoot })
  );
  await fsp.mkdir(safe, { recursive: true });
  return safe;
}


// Preview, confirmation and active-run lock guards (#277, part of #197).
// Clean is dry-run by default; deletion needs execute plus explicit yes, and
// refuses while any repo-locks.mjs lock under the state root has a live holder.
// The locks/ directory is never part of a sweep.

const LOCKS_DIR = 'locks';

// The state root itself is never a deletion target, but sweeps and lock probes read it.
async function canonicalStateRoot(handle) {
  assertStateScope(handle);
  const root = await normalizePath(rootOf(handle));
  if (!root) throw new CleanOpsError('state root handle is missing a root', FORBIDDEN_CODE, { handle });
  return assertNotForbidden(root, await forbiddenStateRoots({ repoRoot: handle.repoRoot, worktreeRoot: handle.worktreeRoot }));
}

function names(value, label) {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.some((name) => typeof name !== 'string' || name.length === 0 ||
      /[\\/]/.test(name) || name === '.' || name === '..')) {
    throw new CleanOpsError(`${label} must be a list of entry names`, SCOPE_CODE);
  }
  return value;
}

async function enumerateRecords(handle, relativeDir, exclude) {
  const root = await canonicalStateRoot(handle);
  const dir = relativeDir && path.resolve(root, relativeDir) !== root ? await resolveStatePath(handle, relativeDir) : root;
  const atRoot = dir === root;
  let entries;
  try {
    entries = await fsp.readdir(dir, { withFileTypes: true });
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ENOTDIR') return { root, dir, included: [], excluded: [] };
    throw error;
  }
  const skip = new Set(exclude);
  const included = [];
  const excluded = [];
  for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
    if (skip.has(entry.name) || (atRoot && entry.name === LOCKS_DIR)) excluded.push(entry.name);
    else included.push({ name: entry.name, kind: entry.isDirectory() ? 'directory' : 'file' });
  }
  return { root, dir, included, excluded };
}

// Read-only listing of lock files that repo-locks.mjs keeps at <state root>/locks/<name>.lock.
export async function listRunLocks(handle) {
  const dir = path.join(await canonicalStateRoot(handle), LOCKS_DIR);
  let entries;
  try {
    entries = await fsp.readdir(dir, { withFileTypes: true });
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ENOTDIR') return [];
    throw error;
  }
  const locks = [];
  for (const entry of entries) {
    if (!entry.isFile() || !entry.name.endsWith('.lock')) continue;
    let holder = null;
    try {
      const record = JSON.parse(await fsp.readFile(path.join(dir, entry.name), 'utf8'));
      if (typeof record?.holder === 'string') holder = record.holder;
    } catch {
      // An unreadable record still counts as a lock; the probe decides liveness.
    }
    locks.push({ name: entry.name.slice(0, -'.lock'.length), holder });
  }
  return locks.sort((a, b) => a.name.localeCompare(b.name));
}

// Probes each lock through acquireRepoLock: a live holder refuses the probe; a dead
// holder's stale lock is taken over and released, exactly as repo-locks.mjs would.
export async function checkActiveRunLock(handle, { isAlive } = {}) {
  const lockRoot = await canonicalStateRoot(handle);
  const held = [];
  for (const { name } of await listRunLocks(handle)) {
    try {
      const lock = await acquireRepoLock(name, { lockRoot, waitMs: 0, holder: `clean-probe-${process.pid}`,
        ...(isAlive ? { isAlive } : {}) });
      await lock.release();
    } catch (error) {
      if (!(error instanceof RepoLockError) || error.code !== 'E_LOCK_HELD') throw error;
      held.push({ name, holder: error.holder ?? null });
    }
  }
  if (held.length) {
    throw new CleanOpsError(`an active run holds ${held.map(({ name }) => `"${name}"`).join(', ')}; ` +
      'refusing to clean until it finishes', LOCK_CODE, { locks: held });
  }
  return { locks: [] };
}

export async function runClean(handle, options = {}) {
  if (!isPlainObject(options)) throw new CleanOpsError('options must be an object', SCOPE_CODE);
  assertStateScope(handle);
  const execute = options.execute === true;
  const yes = options.yes === true;
  const exclude = names(options.exclude, 'exclude');
  if (execute && !yes) {
    throw new CleanOpsError(options.interactive === true
      ? 'destructive clean requires explicit confirmation (--yes)'
      : 'non-interactive destructive clean requires explicit --yes confirmation', CONFIRMATION_CODE);
  }
  const relativeDir = typeof options.relativeDir === 'string' ? options.relativeDir : '';
  const { root, dir, included, excluded } = await enumerateRecords(handle, relativeDir, exclude);
  const report = {
    dryRun: !execute, scope: scopeOf(handle), root,
    included, excluded, recordCount: included.length, activeLocks: await listRunLocks(handle),
  };
  if (!execute) return { ...report, executed: false, removed: [] };
  await checkActiveRunLock(handle, { isAlive: options.isAlive });
  const removed = [];
  for (const { name } of included) {
    removed.push(await removeStatePath(handle, path.relative(root, path.join(dir, name))));
  }
  return { ...report, activeLocks: [], executed: true, removed };
}

export function formatCleanReport(report) {
  if (!isPlainObject(report)) throw new CleanOpsError('formatCleanReport requires a runClean result object', SCOPE_CODE);
  const lines = [
    `mode: ${report.dryRun ? 'dry-run (nothing deleted; rerun with --execute --yes to delete)' : 'destructive'}`,
    `scope: ${report.scope}`,
    `state root: ${report.root}`,
    `records: ${report.recordCount}`,
    ...report.included.map(({ kind, name }) => `  - ${kind} ${name}`),
  ];
  if (report.excluded.length) lines.push(`excluded: ${report.excluded.join(', ')}`);
  if (report.activeLocks.length) {
    lines.push(`active locks: ${report.activeLocks.map(({ name, holder }) => holder ? `${name} (${holder})` : name).join(', ')}`);
  }
  if (report.executed) lines.push(`removed: ${report.removed.length}`);
  return `${lines.join('\n')}\n`;
}
