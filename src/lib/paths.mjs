import { promises as fs, lstatSync, openSync, closeSync, statSync, unlinkSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, relative, resolve, sep, isAbsolute, basename, parse as parsePath, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));

export function resolveProjectRoot(cwd = process.cwd()) {
  const start = resolve(cwd);
  let current = start;
  for (;;) {
    const marker = lstatSync(join(current, '.git'), { throwIfNoEntry: false });
    if (marker) {
      if (marker.isSymbolicLink() || (!marker.isFile() && !marker.isDirectory())) {
        throw new Error('Git worktree marker must be a regular file or directory');
      }
      return current;
    }
    const parent = resolve(current, '..');
    if (parent === current) return start;
    current = parent;
  }
}

export function resolveContractsPath({
  env = process.env,
  cwd = process.cwd(),
  repoRoot = rosterRoot,
} = {}) {
  const configuredPath = env.GITHUB_AGENT_CONTRACTS;

  if (configuredPath !== undefined &&
      (typeof configuredPath !== 'string' || configuredPath.trim() === '')) {
    throw new TypeError('GITHUB_AGENT_CONTRACTS must be a non-empty path');
  }

  const candidates = [resolve(repoRoot, 'vendor', 'github-agent-contracts')];
  if (configuredPath !== undefined) {
    candidates.push(resolve(cwd, configuredPath));
  }
  candidates.push(resolve(repoRoot, '..', 'github-agent-contracts'));

  for (const contractsPath of candidates) {
    const publisher = resolve(contractsPath, 'scripts', 'agent-pr.mjs');
    if (statSync(publisher, { throwIfNoEntry: false })?.isFile()) {
      return contractsPath;
    }
  }

  throw new Error(
    `github-agent-contracts is required: no scripts/agent-pr.mjs file found in ${candidates.join(', ')}. ` +
    'Run git submodule update --init --recursive or set GITHUB_AGENT_CONTRACTS to a complete clone.',
  );
}

export async function ensureLocalPath(file, repoRoot) {
  const root = await fs.realpath(repoRoot);
  const target = resolve(file);
  const rest = relative(root, target);
  if (!rest || rest === '..' || rest.startsWith(`..${sep}`) || isAbsolute(rest)) {
    throw new Error('Path must stay inside the roster repository');
  }
  let current = root;
  for (const part of rest.split(sep)) {
    current = join(current, part);
    let entry;
    try {
      entry = await fs.lstat(current);
    } catch (error) {
      if (error.code === 'ENOENT') break;
      throw error;
    }
    if (entry.isSymbolicLink()) throw new Error('Roster path may not contain symlinks');
  }
}

// ---------------------------------------------------------------------------
// State roots: machine, repository, worktree, and run scopes (issue #194).
// All runtime state consumers resolve paths through these helpers; ad hoc path
// construction and silent fallbacks to the repo or cwd are rejected here.
// ---------------------------------------------------------------------------

export const STATE_SCOPES = ['machine', 'repo', 'worktree', 'run'];

// ---------------------------------------------------------------------------
// Retention policies per state scope (issue #201).
//
// Each scope has a distinct root so machine history, repo state, worktree
// data, and run artifacts are never pruned against the wrong tree. Default
// windows are intentionally per-scope: machine history is the most durable,
// run artifacts the most ephemeral. `optOutSupported` records whether a user
// can fully disable retention for that scope; all scopes currently support
// opt-out.
// ---------------------------------------------------------------------------
export const RETENTION_POLICIES = Object.freeze({
  machine: Object.freeze({
    scope: 'machine',
    root: 'machine',
    defaultWindowMs: 1000 * 60 * 60 * 24 * 90, // 90 days (docs/STATE.md §8.5)
    optOutSupported: true,
  }),
  repo: Object.freeze({
    scope: 'repo',
    root: 'repos',
    defaultWindowMs: 1000 * 60 * 60 * 24 * 14, // 14 days
    optOutSupported: true,
  }),
  worktree: Object.freeze({
    scope: 'worktree',
    root: 'worktrees',
    defaultWindowMs: 1000 * 60 * 60 * 24 * 7,  // 7 days
    optOutSupported: true,
  }),
  run: Object.freeze({
    scope: 'run',
    root: 'runs',
    defaultWindowMs: 1000 * 60 * 60 * 24,      // 1 day
    optOutSupported: true,
  }),
});

export class StateRootError extends Error {
  constructor(message, { scope, path } = {}) {
    super(message);
    this.name = 'StateRootError';
    this.scope = scope;
    if (path !== undefined) this.path = path;
  }
}

export class ContainmentError extends StateRootError {
  constructor(message, { scope, path } = {}) {
    super(message, { scope, path });
    this.name = 'ContainmentError';
    this.code = 'E_CONTAINMENT';
  }
}

export class PathContainmentError extends ContainmentError {
  constructor(message, options = {}) {
    super(message, options);
    this.name = 'PathContainmentError';
  }
}

export class StateScopeError extends StateRootError {
  constructor(message, { scope, path } = {}) {
    super(message, { scope, path });
    this.name = 'StateScopeError';
    this.code = 'E_SCOPE';
  }
}

// Canonical scope vocabulary: 'machine' | 'repo' | 'worktree' | 'run'.
// 'repository' is accepted as a legacy alias wherever a scope is requested.
const SCOPE_ALIASES = { repository: 'repo' };

export function normalizeScope(scope) {
  const canonical = SCOPE_ALIASES[scope] ?? scope;
  if (!STATE_SCOPES.includes(canonical)) {
    throw new StateScopeError(
      `"${String(scope)}" is not a roster state scope; expected one of ${STATE_SCOPES.join(', ')}.`,
      { scope: String(scope) });
  }
  return canonical;
}

export class ScopeMismatchError extends StateRootError {
  constructor(requested, handleScope, path) {
    super(`state handle is scoped to "${handleScope}", but "${requested}" state was requested for ` +
      `${path}. Resolve a "${requested}"-scoped handle instead of reusing the "${handleScope}" one.`,
    { scope: requested, path });
    this.name = 'ScopeMismatchError';
    this.handleScope = handleScope;
  }
}

function keyOf(value, platform) {
  // Windows drives and file names are case-insensitive; POSIX are not.
  return platform === 'win32' ? value.toLowerCase() : value;
}

function pathSegments(value, platform) {
  // Full segment list including the final component, so containment compares
  // whole paths: "C:\\repo" contains "C:\\repo\\a" but never sibling
  // "C:\\repo-x". Comparing only `dir` would treat every sibling under one
  // parent as nested, which silently defeats the state/repo separation.
  const parsed = parsePath(resolve(value));
  const parts = resolve(value).slice(parsed.root.length).split(sep).filter((part) => part !== '');
  return {
    root: keyOf(parsed.root, platform),
    parts: parts.map((part) => keyOf(part, platform)),
  };
}

export function isContainedIn(child, parent, { platform = process.platform } = {}) {
  const childPath = pathSegments(child, platform);
  const parentPath = pathSegments(parent, platform);
  if (childPath.root !== parentPath.root) return false;
  if (parentPath.parts.length > childPath.parts.length) return false;
  // Equal segments means identical paths; a shorter prefix means the child
  // nests inside the parent. Symmetric callers test both directions.
  return parentPath.parts.every((part, index) => part === childPath.parts[index]);
}

function assertNoSymlinkComponents(root, { scope, skipTail = false } = {}) {
  const parts = [];
  let current = resolve(root);
  for (;;) {
    const parsed = parsePath(current);
    if (parsed.base === '') break;
    parts.unshift(parsed.base);
    if (parsed.dir === current) break;
    current = parsed.dir;
  }
  let walked = parsePath(current).root;
  for (const [index, part] of parts.entries()) {
    walked = join(walked, part);
    if (skipTail && index === parts.length - 1) break;
    let status;
    try {
      status = lstatSync(walked, { throwIfNoEntry: false });
    } catch (error) {
      throw new StateRootError(
        `${scope} state path ${walked} could not be inspected (${error.code ?? error.message}); ` +
        'fix permissions or choose another state root.',
        { scope, path: walked });
    }
    if (!status) continue;
    if (status.isSymbolicLink()) {
      throw new ContainmentError(
        `${scope} state path component ${walked} is a symlink or reparse point; roster refuses ` +
        'linked state paths. Remove the link or pick a real directory.',
        { scope, path: walked });
    }
    if (index === parts.length - 1 && !status.isDirectory()) {
      throw new StateRootError(
        `${scope} state root ${walked} exists but is not a directory.`,
        { scope, path: walked });
    }
  }
}

export class StateHandle {
  constructor({ scope, root, repoId, worktreeId, runId, platform, home, writable }) {
    this.scope = scope;
    this.root = root;
    this.repoId = repoId ?? null;
    this.worktreeId = worktreeId ?? null;
    this.runId = runId ?? null;
    this.platform = platform;
    this.home = home;
    this.writable = Boolean(writable);
  }

  // Public summary: never prints the full private path, only its two ends.
  describe() {
    const segments = this.root.split(sep).filter(Boolean);
    const short = segments.length > 2
      ? `${segments[0]}${sep}…${sep}${segments.at(-1)}`
      : this.root;
    return { scope: this.scope, root: short, repoId: this.repoId, worktreeId: this.worktreeId,
      runId: this.runId, writable: this.writable };
  }

  path(...parts) {
    const target = join(this.root, ...parts.map((part) => String(part)));
    const rest = relative(this.root, target);
    if (rest.startsWith('..') || isAbsolute(rest)) {
      throw new PathContainmentError(
        `${this.scope} state path escapes the "${this.scope}" root: ${target}. ` +
        'Use a relative subpath inside the state root.',
        { scope: this.scope, path: target });
    }
    return target;
  }

  scopedPath(requestedScope, ...parts) {
    const canonical = normalizeScope(requestedScope);
    if (canonical !== this.scope) {
      throw new StateScopeError(
        `state handle is scoped to "${this.scope}", but "${canonical}" state was requested for ` +
        `${parts.join('/') || '.'}. Resolve a "${canonical}"-scoped handle instead of reusing the ` +
        `"${this.scope}" one.`,
        { scope: canonical, path: join(this.root, ...parts.map((part) => String(part))) });
    }
    return this.path(...parts);
  }

  machinePath(...parts) {
    return this.scopedPath('machine', ...parts);
  }

  repositoryPath(...parts) {
    return this.scopedPath('repository', ...parts);
  }

  worktreePath(...parts) {
    return this.scopedPath('worktree', ...parts);
  }

  runPath(...parts) {
    return this.scopedPath('run', ...parts);
  }
}

function assertWritableRoot(root, scope) {
  let status;
  try {
    status = statSync(root, { throwIfNoEntry: false });
  } catch (error) {
    throw new StateRootError(
      `${scope} state root ${root} could not be inspected (${error.code ?? error.message}); ` +
      'fix permissions or choose another state root.',
      { scope, path: root });
  }
  if (!status?.isDirectory()) {
    throw new StateRootError(
      `${scope} state root ${root} is missing or not a directory; create it or set ROSTER_STATE_ROOT.`,
      { scope, path: root });
  }
  const probe = join(root, `.roster-probe-${process.pid}-${Date.now()}`);
  let handle;
  try {
    handle = openSync(probe, 'w');
  } catch (error) {
    throw new StateRootError(
      `${scope} state root ${root} is not writable by this process (${error.code ?? error.message}); ` +
      'fix ownership or choose a writable ROSTER_STATE_ROOT.',
      { scope, path: root });
  } finally {
    if (handle !== undefined) closeSync(handle);
    try { unlinkSync(probe); } catch { /* probe cleanup is best effort */ }
  }
  return true;
}

export function resolveMachineRoot({
  env = process.env,
  platform = process.platform,
  home = env.HOME ?? env.USERPROFILE,
} = {}) {
  // Single explicit override key for every platform (issue #194).
  const override = env.PATHS_OVERRIDE ?? env.ROSTER_STATE_ROOT;
  if (override !== undefined) {
    if (typeof override !== 'string' || override.trim() === '') {
      throw new StateRootError(
        `${env.ROSTER_STATE_ROOT !== undefined ? 'ROSTER_STATE_ROOT' : 'PATHS_OVERRIDE'} ` +
        'must be a non-empty path when set.',
        { scope: 'machine', path: String(override) });
    }
    const root = resolve(override);
    assertNoSymlinkComponents(root, { scope: 'machine' });
    assertWritableRoot(root, 'machine');
    return new StateHandle({ scope: 'machine', root, platform, home, writable: true });
  }
  // Platform-native defaults come from the shared resolver, never ad hoc joins.
  const { machine } = platformDefaultRoots({ env, platform, home });
  const root = resolve(machine);
  assertNoSymlinkComponents(root, { scope: 'machine' });
  return new StateHandle({ scope: 'machine', root, platform, home, writable: false });
}

// Identity digests are SHA-1 hex prefixes: stable, dependency-free, and
// short enough to sit in a state directory name alongside a human-readable
// slug of the repository it belongs to.
function hashId(value, length = 16) {
  return createHash('sha1').update(value).digest('hex').slice(0, length);
}

// A filesystem-safe, lowercase slug used as the readable half of a repository
// identity (e.g. "acme/widget" -> "widget"). Slugs never replace the digest;
// they only make state directories identifiable at a glance.
function identitySlug(value) {
  const slug = String(value ?? '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 32);
  return slug === '' ? 'repo' : slug;
}

function remoteName(remote) {
  const withoutSuffix = remote.replace(/\.git$/i, '');
  const segments = withoutSuffix.split(/[\\/:]/).filter((part) => part !== '');
  return segments.at(-1) ?? 'repo';
}

// Platform-native default state-root directories. These are the only
// supported defaults; nothing silently falls back to the repo or cwd.
export function platformDefaultRoots({
  env = process.env,
  platform = process.platform,
  home = env.HOME ?? env.USERPROFILE,
} = {}) {
  if (platform === 'win32') {
    const localAppData = resolve(env.LOCALAPPDATA ?? join(home ?? '', 'AppData', 'Local'));
    return { machine: join(localAppData, 'roster', 'state') };
  }
  if (platform === 'darwin') {
    return { machine: join(home ?? '', 'Library', 'Application Support', 'roster') };
  }
  return {
    machine: join(resolve(env.XDG_STATE_HOME ?? join(home ?? '', '.local', 'state')), 'roster'),
  };
}

export function repositoryId({ repoRoot, env = process.env } = {}) {
  if (!repoRoot) {
    throw new StateRootError('repository identity requires a repository root.',
      { scope: 'repository' });
  }
  // Normalize to the repository root found by walking up for the git marker, so
  // identity does not depend on which subdirectory (worktree, nested package)
  // the caller happened to start from.
  const root = resolveProjectRoot(resolve(repoRoot));
  const remote = typeof env.GIT_REMOTE_URL === 'string' && env.GIT_REMOTE_URL.trim() !== ''
    ? env.GIT_REMOTE_URL.trim()
    : null;
  // Remote URL preferred: same repository stays identifiable across checkouts.
  // Offline/no-remote: fall back to the checkout's device/inode pair so two
  // checkouts of one project never collide with each other or with a remote.
  // The slug (from the remote name, or the checkout directory when offline) is
  // cosmetic; the SHA-1 digest carries the identity.
  if (remote) return `${identitySlug(remoteName(remote))}-${hashId(`remote:${remote}`)}`;
  const status = statSync(root);
  const slug = identitySlug(basename(root));
  return `${slug}-${hashId(`checkout:${status.dev}:${status.ino}`)}`;
}

export function resolveRepositoryRoot({ cwd = process.cwd() } = {}) {
  const repoRoot = resolveProjectRoot(cwd);
  // Every component including the tail is checked: a symlinked repository root
  // would let state resolve to a different real directory than requested.
  assertNoSymlinkComponents(repoRoot, { scope: 'repository' });
  return repoRoot;
}

function assertStateOutsideRepository(stateRoot, repoRoot) {
  if (isContainedIn(stateRoot, repoRoot) || isContainedIn(repoRoot, stateRoot)) {
    throw new PathContainmentError(
      `repository state root ${stateRoot} must stay outside the repository ${repoRoot}; ` +
      'set PATHS_OVERRIDE to a machine-scoped directory.',
      { scope: 'repository', path: stateRoot });
  }
}

export function resolveRepositoryState({
  cwd = process.cwd(),
  env = process.env,
  repoRoot: providedRoot,
  machineRoot,
} = {}) {
  const repoRoot = providedRoot ?? resolveRepositoryRoot({ cwd });
  const machine = machineRoot ?? resolveMachineRoot({ env });
  const id = repositoryId({ repoRoot, env });
  const stateRoot = join(machine.root, 'repos', id);
  assertStateOutsideRepository(stateRoot, repoRoot);
  return new StateHandle({
    scope: 'repository',
    root: stateRoot,
    repoId: id,
    platform: machine.platform,
    home: machine.home,
    writable: true,
  });
}

export function worktreeId(repoRoot, worktreeRoot) {
  if (!repoRoot || !worktreeRoot) {
    throw new StateRootError('worktree identity requires both a repository and a worktree root.',
      { scope: 'worktree' });
  }
  return `wt-${hashId(`${resolve(repoRoot)}::${resolve(worktreeRoot)}`)}`;
}

export function resolveWorktreeState({
  repoRoot,
  worktreeRoot,
  env = process.env,
  machineRoot,
} = {}) {
  if (!repoRoot || !worktreeRoot) {
    throw new StateRootError(
      'worktree state requires both repoRoot and worktreeRoot; resolve the repository root first.',
      { scope: 'worktree' });
  }
  const repository = resolveRepositoryState({ repoRoot, env, machineRoot });
  const id = worktreeId(repoRoot, worktreeRoot);
  return new StateHandle({
    scope: 'worktree',
    root: join(repository.root, 'worktrees', id),
    repoId: repository.repoId,
    worktreeId: id,
    platform: repository.platform,
    home: repository.home,
    writable: true,
  });
}

export function newRunId({ name = 'run', timestamp = Date.now(), random } = {}) {
  const suffix = random ?? Math.random().toString(36).slice(2, 10);
  return `${name}-${timestamp.toString(36)}-${suffix}`;
}

export function resolveRunState({
  repoRoot,
  worktreeRoot,
  runIdentifier,
  env = process.env,
  machineRoot,
} = {}) {
  const worktree = resolveWorktreeState({ repoRoot, worktreeRoot, env, machineRoot });
  const identifier = runIdentifier ?? newRunId();
  if (!/^[A-Za-z0-9._-]+$/.test(identifier)) {
    throw new StateRootError(
      `run id "${identifier}" may only contain letters, digits, dot, dash, and underscore.`,
      { scope: 'run', path: identifier });
  }
  return new StateHandle({
    scope: 'run',
    root: join(worktree.root, 'runs', identifier),
    repoId: worktree.repoId,
    worktreeId: worktree.worktreeId,
    runId: identifier,
    platform: worktree.platform,
    home: worktree.home,
    writable: true,
  });
}

export async function ensureStateRoot(handle) {
  if (!(handle instanceof StateHandle)) {
    throw new StateRootError('ensureStateRoot requires a state handle returned by a resolver.',
      { scope: 'machine' });
  }
  await fs.mkdir(handle.root, { recursive: true, mode: 0o700 });
  return handle.root;
}

// ---------------------------------------------------------------------------
// Canonical containment, atomic directory creation, and lock ownership.
// ---------------------------------------------------------------------------

export async function canonicalPath(target, { scope = 'run' } = {}) {
  if (typeof target !== 'string' || target.trim() === '') {
    throw new PathContainmentError('canonical path requires a non-empty string.', { scope, path: target });
  }
  try {
    return await fs.realpath(resolve(target));
  } catch (error) {
    if (error.code === 'ENOENT') {
      // Only the tail may be missing; every parent must already be a real dir.
      await assertRealParents(resolve(target), { scope });
      return resolve(target);
    }
    throw new StateRootError(
      `${scope} path ${target} could not be canonically resolved (${error.code ?? error.message}).`,
      { scope, path: String(target) });
  }
}

async function assertRealParents(target, { scope }) {
  let current = dirname(target);
  const walked = [];
  for (;;) {
    let status;
    try {
      status = await fs.lstat(current);
    } catch (error) {
      if (error.code === 'ENOENT') {
        walked.unshift(current);
        const parent = dirname(current);
        if (parent === current) break;
        current = parent;
        continue;
      }
      throw new StateRootError(
        `${scope} path ${current} could not be inspected (${error.code ?? error.message}).`,
        { scope, path: current });
    }
    if (status.isSymbolicLink()) {
      throw new PathContainmentError(
        `${scope} path component ${current} is a symlink or reparse point; remove the link or pick a real directory.`,
        { scope, path: current });
    }
    break;
  }
}

export async function ensureContainedPath(root, relativeTarget, { scope = 'run' } = {}) {
  const canonicalRoot = await fs.realpath(resolve(root));
  const target = resolve(canonicalRoot, relativeTarget);
  const inside = relative(canonicalRoot, target);
  if (!inside || inside === '..' || inside.startsWith(`..${sep}`) || isAbsolute(inside)) {
    throw new PathContainmentError(
      `${scope} path ${target} escapes the canonical root ${canonicalRoot}; use a subpath inside it.`,
      { scope, path: target });
  }
  let current = canonicalRoot;
  for (const part of inside.split(sep)) {
    current = join(current, part);
    const status = await fs.lstat(current).catch((error) => {
      if (error.code === 'ENOENT') return null;
      throw new StateRootError(
        `${scope} path ${current} could not be inspected (${error.code ?? error.message}).`,
        { scope, path: current });
    });
    if (status?.isSymbolicLink()) {
      throw new PathContainmentError(
        `${scope} path component ${current} is a symlink or reparse point; roster refuses linked state paths.`,
        { scope, path: current });
    }
  }
  return { root: canonicalRoot, target };
}

export async function atomicMkdir(target, { mode = 0o700, recursive = false } = {}) {
  if (typeof target !== 'string' || target.trim() === '') {
    throw new StateRootError('atomicMkdir requires a non-empty directory path.', { scope: 'run', path: target });
  }
  // mkdir with recursive: true is a no-op when the tree already exists, so it
  // is the idempotent path; a single missing parent is created otherwise.
  if (recursive) {
    await fs.mkdir(target, { recursive: true, mode });
  } else {
    await fs.mkdir(dirname(target), { recursive: true, mode });
    try {
      await fs.mkdir(target, { mode });
    } catch (error) {
      if (error.code !== 'EEXIST') throw error;
      // Idempotent: an existing directory is fine, anything else is a failure.
    }
  }
  await fs.chmod(target, mode);
  return target;
}

function pidAlive(pid) {
  if (!Number.isSafeInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === 'EPERM';
  }
}

export async function acquireLock(target, {
  runId = null,
  mode = 0o700,
  waitMs = 0,
  pollMs = 25,
  clock = () => Date.now(),
  isAlive = pidAlive,
  // Test seam: the production unlink is fs.unlink; the injection lets tests
  // prove a lock file is only ever removed after the liveness probe says the
  // recorded owner is dead. It never changes the production default.
  unlink = (file) => fs.unlink(file),
} = {}) {
  if (typeof target !== 'string' || target.trim() === '') {
    throw new StateRootError('acquireLock requires a non-empty lock path.', { scope: 'run', path: target });
  }
  await atomicMkdir(dirname(target), { recursive: true });
  const deadline = clock() + waitMs;
  for (;;) {
    let handle;
    try {
      // fs.promises.open(path, flags, mode) is exclusive-create (O_EXCL via
      // 'wx') and rejects with EEXIST when another owner holds the lock.
      handle = await fs.open(target, 'wx', mode);
    } catch (error) {
      if (error.code !== 'EEXIST') {
        throw new StateRootError(
          `lock ${target} could not be created (${error.code ?? error.message}).`,
          { scope: 'run', path: target });
      }
      // Steal only when the recorded owner is provably dead (crash recovery).
      let existing;
      try {
        existing = JSON.parse(await fs.readFile(target, 'utf8'));
      } catch (error) {
        // The owner released the lock between our failed create and this read:
        // the next create attempt succeeds, no stealing is involved.
        if (error.code === 'ENOENT') continue;
        // Unreadable/malformed JSON: the owner is unknown, so no liveness
        // probe can prove it dead. Never drop a lock we cannot attribute.
        existing = null;
      }
      const ownerPid = existing && Number.isSafeInteger(existing.pid) && existing.pid > 0
        ? existing.pid
        : null;
      if (ownerPid === null) {
        throw new StateRootError(
          `lock ${target} exists but does not record a valid owner pid; refusing to steal it ` +
          'without proof the owning process is dead. Inspect and remove it manually once the ' +
          'owner is known to be gone.',
          { scope: 'run', path: target });
      }
      if (!isAlive(ownerPid)) {
        // Liveness probe proved the recorded owner dead: the lock is stale and
        // may be stolen. EACCES/EPERM on the unlink keeps the lock in place so
        // the error surfaces instead of silently taking the lock anyway.
        try {
          await unlink(target);
        } catch (error) {
          if (error.code !== 'ENOENT') {
            throw new StateRootError(
              `stale lock ${target} could not be removed (${error.code ?? error.message}); ` +
              'remove it manually once the owner is known to be gone.',
              { scope: 'run', path: target });
          }
        }
        continue;
      }
      if (clock() >= deadline) {
        throw new StateRootError(
          `lock ${target} is held by a live process (pid ${existing.pid}, run ${existing.runId ?? 'unknown'}); ` +
          'wait for release or remove the stale lock once the owner is gone.',
          { scope: 'run', path: target });
      }
      await new Promise((resolveSleep) => setTimeout(resolveSleep, pollMs));
      continue;
    }
    const lock = { pid: process.pid, runId, acquiredAt: new Date().toISOString() };
    try {
      await handle.write(`${JSON.stringify(lock)}\n`, null, 'utf8');
    } finally {
      await handle.close();
    }
    return { ...lock, path: target, release };
    async function release() {
      return releaseLock(target, { pid: process.pid, runId });
    }
  }
}

export async function releaseLock(target, { pid = process.pid, runId } = {}) {
  let existing = null;
  try {
    existing = JSON.parse(await fs.readFile(target, 'utf8'));
  } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw new StateRootError(
      `lock ${target} could not be read for release (${error.code ?? error.message}).`,
      { scope: 'run', path: target });
  }
  if (!existing || existing.pid !== pid || (runId !== undefined && existing.runId !== runId)) return false;
  await fs.rm(target, { force: true });
  return true;
}

export async function withLock(target, operation, options = {}) {
  const lock = await acquireLock(target, options);
  try {
    return await operation(lock);
  } finally {
    await lock.release();
  }
}

// ---------------------------------------------------------------------------
// Public resolver API: the single shared way to obtain machine, repository,
// worktree, and run scoped paths. Existing exports stay available unchanged.
// ---------------------------------------------------------------------------

export function resolveRepositoryIdentity(repoRoot, { env = process.env } = {}) {
  const id = repositoryId({ repoRoot, env });
  const identity = { repoRoot: resolve(repoRoot), repoId: id };
  if (env.ROSTER_REPO_ID !== undefined) {
    if (typeof env.ROSTER_REPO_ID !== 'string' || env.ROSTER_REPO_ID.trim() === '') {
      throw new StateRootError('ROSTER_REPO_ID must be a non-empty repository id when set.',
        { scope: 'repository' });
    }
    identity.repoId = env.ROSTER_REPO_ID.trim();
  }
  return identity;
}

export async function resolveStateRoot({ env = process.env, repoRoot, machineRoot } = {}) {
  if (!repoRoot) throw new StateRootError('resolveStateRoot requires a repository root.',
    { scope: 'repository' });
  const machine = await machineHandle(machineRoot, { env });
  const { repoId } = resolveRepositoryIdentity(repoRoot, { env });
  // Canonicalize both ends before comparing: two spellings of the same
  // directory (symlink, 8.3 name, case) must not defeat separation.
  const canonicalRepo = await canonicalPath(repoRoot, { scope: 'repository' });
  const stateRoot = join(machine.root, 'repos', repoId);
  assertNoSymlinkComponents(machine.root, { scope: 'machine' });
  assertStateOutsideRepository(stateRoot, canonicalRepo);
  return new StateHandle({
    scope: 'repo', root: stateRoot, repoId,
    platform: machine.platform, home: machine.home, writable: true,
  });
}

// Accepts an already-resolved state handle, a plain root descriptor, or a root
// path string. Ad-hoc roots are held to the same rules as resolveMachineRoot:
// no symlink/reparse component may appear anywhere in the supplied root. An
// unrecognized value is refused rather than silently resolving the real
// user machine root (#361).
async function machineHandle(machineRoot, { env }) {
  if (machineRoot instanceof StateHandle) return machineRoot;
  if (typeof machineRoot === 'string') machineRoot = { root: machineRoot, writable: true };
  if (machineRoot && typeof machineRoot.root === 'string') {
    assertNoSymlinkComponents(machineRoot.root, { scope: 'machine' });
    // Canonicalize so containment compares real directories: a symlinked or
    // aliased spelling of the repository cannot smuggle state inside it.
    const root = await canonicalPath(machineRoot.root, { scope: 'machine' });
    return {
      ...machineRoot,
      root,
      platform: machineRoot.platform ?? process.platform,
      writable: Boolean(machineRoot.writable),
    };
  }
  if (machineRoot != null) {
    throw new StateRootError(
      'machineRoot must be a StateHandle, a { root } descriptor, or a root path string.',
      { scope: 'machine' });
  }
  return resolveMachineRoot({ env });
}


export async function resolveWorktreeRoot({ repoRoot, worktreeRoot, env = process.env, machineRoot } = {}) {
  if (!repoRoot || !worktreeRoot) {
    throw new StateRootError(
      'resolveWorktreeRoot requires both repoRoot and worktreeRoot; resolve the repository root first.',
      { scope: 'worktree' });
  }
  const repository = await resolveStateRoot({ env, repoRoot, machineRoot });
  const id = worktreeId(repoRoot, worktreeRoot);
  return new StateHandle({
    scope: 'worktree',
    root: join(repository.root, 'worktrees', id),
    repoId: repository.repoId,
    worktreeId: id,
    platform: repository.platform,
    home: repository.home,
    writable: true,
  });
}

export async function resolveRunPaths({
  repoRoot,
  worktreeRoot,
  runIdentifier,
  env = process.env,
  machineRoot,
} = {}) {
  const worktree = await resolveWorktreeRoot({ repoRoot, worktreeRoot, env, machineRoot });
  const identifier = runIdentifier ?? newRunId();
  if (!/^[A-Za-z0-9._-]+$/.test(identifier)) {
    throw new StateRootError(
      `run id "${identifier}" may only contain letters, digits, dot, dash, and underscore.`,
      { scope: 'run', path: identifier });
  }
  const runRoot = join(worktree.root, 'runs', identifier);
  return {
    runId: identifier,
    runRoot,
    stateRoot: worktree.root,
    repoId: worktree.repoId,
    worktreeId: worktree.worktreeId,
    paths: {
      root: runRoot,
      log: join(runRoot, 'run.log'),
      lock: join(runRoot, 'run.lock'),
      memory: join(runRoot, 'memory.jsonl'),
      events: join(runRoot, 'events.jsonl'),
    },
    handle: new StateHandle({
      scope: 'run', root: runRoot, repoId: worktree.repoId,
      worktreeId: worktree.worktreeId, runId: identifier,
      platform: worktree.platform, home: worktree.home, writable: true,
    }),
  };
}
