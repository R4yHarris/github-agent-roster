import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { randomBytes } from 'node:crypto';
import { StateHandle, resolveRepositoryRoot, resolveStateRoot } from './paths.mjs';
import { compareIdentity, resolveRepoIdentity } from './repo-identity.mjs';

export const SCHEMA_MARKER = 'schema.json';
export const IDENTITY_FILE = 'identity';
export const CURRENT_SCHEMA_VERSION = 1;

export class RepoStateError extends Error {
  constructor(message, { code, path, cause } = {}) {
    super(message);
    this.name = 'RepoStateError';
    if (code !== undefined) this.code = code;
    if (path !== undefined) this.path = path;
    if (cause !== undefined) this.cause = cause;
  }
}

function markerPath(stateRoot) {
  return join(stateRoot, SCHEMA_MARKER);
}

function identityPath(stateRoot) {
  return join(stateRoot, IDENTITY_FILE);
}

function recoveryHint(stateRoot) {
  return `Recover by deleting the state directory ${stateRoot} and reopening it ` +
    `(POSIX: rm -rf "${stateRoot}"; PowerShell: Remove-Item -Recurse -Force "${stateRoot}").`;
}

function leftoverTempFiles(entries, base) {
  return entries.filter((entry) => entry.startsWith(`${base}.`) && entry.endsWith('.tmp'));
}

/**
 * Open (and if needed initialize) a repo-state directory.
 *
 * `stateRoot` must be the root produced by the #198 path resolver
 * (e.g. `resolveRepositoryState(...).root`, a `StateHandle`, or the result of
 * `resolveRepoState()`).
 *
 * - First use: creates the dir, writes the schema marker and the derived
 *   repo identity (hashed, never plaintext).
 * - Subsequent use: verifies marker version and identity; mismatches fail
 *   closed with an actionable error and never reinitialize silently.
 * - Fresh clone: a missing state dir is created with an empty state object.
 */
export async function openRepoState(stateRoot, {
  identity,
  deriveIdentity,
  // Documented policy for identity mismatches: 'refuse' (default) fails closed
  // and keeps the recorded state; 'reinitialize' recreates the marker and
  // records the new identity. Any other value is rejected, never guessed at.
  onIdentityMismatch = 'refuse',
  // Test seam for atomic-write failure injection; production default is fs.
  fileSystem = fs,
} = {}) {
  if (typeof stateRoot !== 'string' || stateRoot.trim() === '') {
    throw new RepoStateError('openRepoState requires a state root path from the path resolver.',
      { code: 'E_STATE_ROOT' });
  }
  if (onIdentityMismatch !== 'refuse' && onIdentityMismatch !== 'reinitialize') {
    throw new RepoStateError(
      `unknown identity-mismatch policy ${JSON.stringify(onIdentityMismatch)}; ` +
      'expected "refuse" or "reinitialize".',
      { code: 'E_IDENTITY_POLICY', path: stateRoot });
  }
  const resolvedIdentity = identity ?? (deriveIdentity ? await deriveIdentity() : undefined);

  let marker;
  try {
    marker = await readMarker(stateRoot, { fileSystem });
  } catch (error) {
    if (error.code !== 'E_NO_STATE_DIR') throw error;
    // Fresh clone: no state dir yet. Create it and write the marker.
    await fileSystem.mkdir(stateRoot, { recursive: true, mode: 0o700 });
    await writeMarker(stateRoot, CURRENT_SCHEMA_VERSION, { fileSystem });
    if (resolvedIdentity !== undefined) {
      await writeIdentity(stateRoot, resolvedIdentity, { fileSystem });
    }
    return { stateRoot, state: {}, marker: { version: CURRENT_SCHEMA_VERSION }, initialized: true };
  }

  if (resolvedIdentity !== undefined) {
    let recorded = null;
    try {
      recorded = (await fileSystem.readFile(identityPath(stateRoot), 'utf8')).trim();
    } catch (error) {
      if (error.code !== 'ENOENT') {
        throw new RepoStateError(
          `recorded repository identity ${identityPath(stateRoot)} could not be read ` +
          `(${error.code ?? error.message}). ${recoveryHint(stateRoot)}`,
          { code: 'E_IDENTITY_UNREADABLE', path: identityPath(stateRoot), cause: error });
      }
    }
    const verdict = compareIdentity(recorded, resolvedIdentity);
    if (verdict.status === 'mismatch') {
      // An actual identity mismatch follows the documented policy: refuse by
      // default (state stays intact), or re-initialize when explicitly asked.
      if (onIdentityMismatch !== 'reinitialize') {
        throw new RepoStateError(
          `repository identity mismatch in ${stateRoot}: recorded ${verdict.recorded}, ` +
          `current ${verdict.current}. ${verdict.message}`,
          { code: 'E_IDENTITY_MISMATCH', path: stateRoot });
      }
      await writeMarker(stateRoot, CURRENT_SCHEMA_VERSION, { fileSystem });
      await writeIdentity(stateRoot, resolvedIdentity, { fileSystem });
      return {
        stateRoot,
        state: {},
        marker: { version: CURRENT_SCHEMA_VERSION },
        identity: { reinitialized: true, from: verdict.recorded, to: verdict.current },
      };
    }
    // No identity on record yet: record it on this open (initialize path).
    if (verdict.status === 'initialize') {
      await writeIdentity(stateRoot, resolvedIdentity, { fileSystem });
    }
  }
  return { stateRoot, state: {}, marker };
}

/**
 * Resolve the repo-scoped state root through the #198 resolver and open it.
 * `stateRoot` may be supplied directly (a `StateHandle` or path) so a caller
 * that already resolved a root does not resolve twice.
 */
export async function resolveRepoState({
  repoRoot,
  cwd = process.cwd(),
  env = process.env,
  machineRoot,
  stateRoot,
  identity,
  deriveIdentity,
  onIdentityMismatch,
  fileSystem = fs,
} = {}) {
  let root = stateRoot;
  let identityRoot = repoRoot;
  if (root instanceof StateHandle) {
    // The #198 resolver already produced this root; reuse it verbatim.
    root = root.root;
  }
  // A plain string machine root would be ignored by the resolver (which only
  // accepts a handle or a root descriptor), silently falling back to the real
  // user state dir. Normalize it so an explicit root is always honored.
  const machine = typeof machineRoot === 'string'
    ? { root: machineRoot, writable: true }
    : machineRoot;
  if (typeof root !== 'string' || root.trim() === '') {
    const rootForRepo = repoRoot ?? resolveRepositoryRoot({ cwd });
    identityRoot = rootForRepo;
    const handle = await resolveStateRoot({ env, repoRoot: rootForRepo, machineRoot: machine });
    root = handle.root;
  }
  const derive = deriveIdentity ?? (identityRoot
    ? () => resolveRepoIdentity({ repoRoot: identityRoot })
    : undefined);
  return openRepoState(root, { identity, deriveIdentity: derive, onIdentityMismatch, fileSystem });
}

export async function readMarker(stateRoot, { fileSystem = fs } = {}) {
  const file = markerPath(stateRoot);
  let entries;
  try {
    entries = await fileSystem.readdir(stateRoot);
  } catch (error) {
    if (error.code === 'ENOENT') {
      throw new RepoStateError('state directory does not exist yet.',
        { code: 'E_NO_STATE_DIR', path: stateRoot });
    }
    throw new RepoStateError(
      `state directory ${stateRoot} could not be read (${error.code ?? error.message}). ${recoveryHint(stateRoot)}`,
      { code: 'E_STATE_ROOT', path: stateRoot, cause: error });
  }
  if (!entries.includes(SCHEMA_MARKER)) {
    const leftovers = leftoverTempFiles(entries, SCHEMA_MARKER);
    if (leftovers.length) {
      // An interrupted marker write: the previous marker is gone and a temp
      // file remains. Treat that as a broken state, never as "empty".
      throw new RepoStateError(
        `schema marker ${file} is missing but an interrupted atomic write left ${leftovers.join(', ')} ` +
        `behind, so the state was not initialized cleanly. ${recoveryHint(stateRoot)}`,
        { code: 'E_INTERRUPTED_WRITE', path: file });
    }
    throw new RepoStateError(
      `schema marker ${file} is missing: the state directory exists but was not initialized by this ` +
      `version (expected schema version ${CURRENT_SCHEMA_VERSION}, found none). ${recoveryHint(stateRoot)}`,
      { code: 'E_MARKER_MISSING', path: file });
  }
  let parsed;
  try {
    parsed = JSON.parse(await fileSystem.readFile(file, 'utf8'));
  } catch (error) {
    throw new RepoStateError(
      `schema marker ${file} is unreadable or corrupt (${error.message}). ` +
      `Expected version ${CURRENT_SCHEMA_VERSION}, found an unparseable file. ${recoveryHint(stateRoot)}`,
      { code: 'E_MARKER_CORRUPT', path: file, cause: error });
  }
  const found = parsed?.version;
  if (found !== CURRENT_SCHEMA_VERSION) {
    throw new RepoStateError(
      `schema marker ${file} has version ${JSON.stringify(found)} but this build expects ` +
      `${CURRENT_SCHEMA_VERSION}. ${recoveryHint(stateRoot)}`,
      { code: 'E_MARKER_VERSION', path: file });
  }
  return parsed;
}

async function writeMarker(stateRoot, version, { fileSystem = fs } = {}) {
  await atomicWriteFile(markerPath(stateRoot), `${JSON.stringify({ version }, null, 2)}\n`, { fileSystem });
}

async function writeIdentity(stateRoot, identity, { fileSystem = fs } = {}) {
  await atomicWriteFile(identityPath(stateRoot), `${identity}\n`, { fileSystem });
}

/**
 * Atomic write: temp file in the same directory, fsync, then rename.
 * A failure before the rename leaves the previous contents readable and no
 * temp file behind; a failure during the rename leaves the previous contents
 * readable and the temp file behind, which `readRepoStateFile` and
 * `openRepoState` detect on the next use instead of silently reinitializing.
 */
export async function atomicWriteFile(file, data, {
  mode = 0o600,
  fileSystem = fs,
  rename,
} = {}) {
  const fsImpl = { ...fs, ...fileSystem };
  const renameImpl = rename ?? fsImpl.rename;
  const temporary = `${file}.${randomBytes(8).toString('hex')}.tmp`;
  let handle;
  try {
    handle = await fsImpl.open(temporary, 'wx', mode);
    await handle.writeFile(data, 'utf8');
    // fsync is best-effort: some injected/test handles do not implement it.
    if (typeof handle.sync === 'function') await handle.sync();
  } catch (error) {
    await handle?.close().catch(() => {});
    await fsImpl.rm(temporary, { force: true }).catch(() => {});
    throw error;
  }
  try {
    await handle.close();
  } catch (error) {
    await fsImpl.rm(temporary, { force: true }).catch(() => {});
    throw error;
  }
  try {
    await renameImpl(temporary, file);
  } catch (error) {
    throw new RepoStateError(
      `atomic write of ${file} failed during the final rename (${error.code ?? error.message}); ` +
      `the previous contents of ${file} (if any) are untouched and ${temporary} was left in place ` +
      'so the interruption is detected on the next open.',
      { code: 'E_ATOMIC_RENAME', path: file, cause: error });
  }
  return file;
}

/**
 * Read a state file written by atomicWriteFile. Fails closed when the target
 * is missing while a temp file (an interrupted write) remains, instead of
 * treating the state as empty.
 */
export async function readRepoStateFile(file, { fileSystem = fs } = {}) {
  try {
    return await fileSystem.readFile(file, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') {
      const base = file.split(/[\\/]/).pop();
      const dir = file.slice(0, file.length - base.length - 1) || '.';
      const siblings = await fileSystem.readdir(dir).catch(() => []);
      if (leftoverTempFiles(siblings, base).length) {
        throw new RepoStateError(
          `state file ${file} is missing but an interrupted atomic write left a temp file behind; ` +
          'refusing to treat the state as empty. Remove the state directory to re-initialize.',
          { code: 'E_INTERRUPTED_WRITE', path: file });
      }
    }
    throw error;
  }
}

/**
 * Convenience wrapper: write a JSON-serializable state value atomically.
 */
export async function writeRepoStateFile(file, value, options = {}) {
  return atomicWriteFile(file, `${JSON.stringify(value, null, 2)}\n`, options);
}
