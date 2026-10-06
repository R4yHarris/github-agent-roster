import { promises as fs } from 'node:fs';
import path from 'node:path';
import { STATE_SCOPES, statePaths, LEGACY_STATE_DIRNAME } from './repo-state.mjs';

/**
 * Atomic lock acquisition over the repo-state API (issue #196 wave 2).
 *
 * Locks are per-worktree scoped state: the lock file is created exclusively
 * (O_EXCL via 'wx'), so a second concurrent writer is refused with an error
 * naming both the lock and the recorded holder. A stale lock — one whose
 * recorded holder pid is provably dead — is taken over instead.
 */

const defaultIsAlive = (pid) => {
  if (!Number.isSafeInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    // EPERM means the process exists but is not ours; it is alive.
    return error.code === 'EPERM';
  }
};

export class RepoLockError extends Error {
  constructor(message, { code, lock, holder, cause } = {}) {
    super(message);
    this.name = 'RepoLockError';
    if (code !== undefined) this.code = code;
    if (lock !== undefined) this.lock = lock;
    if (holder !== undefined) this.holder = holder;
    if (cause !== undefined) this.cause = cause;
  }
}

/**
 * Acquire the named lock atomically.
 *
 * - `name` must be a filesystem-safe opaque lock identifier.
 * - `repoRoot`/`worktreeRoot` resolve the per-worktree lock directory.
 * - `staleMs` bounds how old a lock may be before its holder is probed for
 *   liveness; a lock whose holder is provably dead is taken over.
 */
export async function acquireRepoLock(name, {
  repoRoot,
  worktreeRoot,
  holder = `pid-${process.pid}`,
  waitMs = 0,
  staleMs = 30_000,
  pollMs = 25,
  clock = () => Date.now(),
  isAlive = defaultIsAlive,
  fileSystem = fs,
  now = () => new Date().toISOString(),
} = {}) {
  if (typeof name !== 'string' || !/^[A-Za-z0-9._-]{1,64}$/.test(name)) {
    throw new RepoLockError('Lock name must be an opaque identifier of at most 64 characters.');
  }
  const resolved = await statePaths({
    scope: STATE_SCOPES.PER_WORKTREE,
    repoRoot,
    worktreeRoot,
    layoutDirName: LEGACY_STATE_DIRNAME,
    segments: ['locks', `${name}.lock`],
    fileSystem,
  });
  // The lock file's parent (locks/) must exist before the exclusive create;
  // mkdir is idempotent so concurrent acquirers never race on it.
  await fileSystem.mkdir(path.dirname(resolved.path), { recursive: true, mode: 0o700 });
  const deadline = clock() + waitMs;
  for (;;) {
    const record = { holder, pid: process.pid, acquiredAt: now() };
    let handle;
    try {
      // Exclusive create ('wx'): the atomicity guarantee. Exactly one writer
      // successfully creates the lock file; every other concurrent writer
      // rejects with EEXIST in the same instant.
      handle = await fileSystem.open(resolved.path, 'wx', 0o600);
    } catch (error) {
      if (error.code !== 'EEXIST') {
        throw new RepoLockError(`Lock ${name} could not be created (${error.code ?? error.message}).`,
          { code: 'E_LOCK_CREATE', lock: name, cause: error });
      }
      // The lock exists: identify the holder before any refusal or takeover.
      let existing = null;
      try {
        existing = JSON.parse(await fileSystem.readFile(resolved.path, 'utf8'));
      } catch (readError) {
        if (readError.code !== 'ENOENT') {
          existing = null;
        }
      }
      const existingHolder = existing && typeof existing.holder === 'string' ? existing.holder : null;
      const acquiredAt = existing && typeof existing.acquiredAt === 'string' ? existing.acquiredAt : null;
      const age = acquiredAt !== null && Number.isFinite(Date.parse(acquiredAt))
        ? clock() - Date.parse(acquiredAt)
        : null;
      const pid = existing && Number.isSafeInteger(existing.pid) && existing.pid > 0 ? existing.pid : null;
      // Stale-lock policy: a lock whose recorded holder is provably dead, or
      // that has exceeded the stale bound with no live holder, is taken over.
      const stale = (pid !== null && !isAlive(pid)) ||
        (age !== null && age > staleMs && (pid === null || !isAlive(pid)));
      if (!stale) {
        if (clock() >= deadline) {
          throw new RepoLockError(
            `lock "${name}" is held by ${existingHolder ?? 'an unknown holder'}; concurrent ` +
            'writers are refused. Wait for release or remove the stale lock once its owner is gone.',
            { code: 'E_LOCK_HELD', lock: name, holder: existingHolder });
        }
        await new Promise((resolveSleep) => setTimeout(resolveSleep, pollMs));
        continue;
      }
      // Takeover: remove the provably dead holder's lock and retry. A failed
      // unlink keeps the lock in place so the error surfaces instead of
      // silently taking the lock anyway.
      try {
        await fileSystem.unlink(resolved.path);
      } catch (unlinkError) {
        if (unlinkError.code !== 'ENOENT') {
          throw new RepoLockError(
            `stale lock "${name}" could not be taken over (${unlinkError.code ?? unlinkError.message}).`,
            { code: 'E_LOCK_STALE', lock: name, cause: unlinkError });
        }
      }
      // Loop again: the exclusive create either succeeds now or another
      // writer took the lock first, in which case the holder check repeats.
      continue;
    }
    try {
      await handle.writeFile(`${JSON.stringify(record)}\n`, 'utf8');
      await handle.close();
      handle = undefined;
    } catch (writeError) {
      await handle?.close().catch(() => {});
      await fileSystem.rm(resolved.path, { force: true }).catch(() => {});
      throw new RepoLockError(`Lock ${name} could not record its holder (${writeError.code ?? writeError.message}).`,
        { code: 'E_LOCK_RECORD', lock: name, cause: writeError });
    }
    return {
      name, path: resolved.path, holder, acquiredAt: record.acquiredAt,
      release: () => releaseRepoLock(resolved.path, { holder, fileSystem }),
    };
  }
}

async function releaseRepoLock(target, { holder, fileSystem }) {
  let existing = null;
  try {
    existing = JSON.parse(await fileSystem.readFile(target, 'utf8'));
  } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw new RepoLockError(`Lock ${target} could not be read for release (${error.code ?? error.message}).`,
      { code: 'E_LOCK_RELEASE', cause: error });
  }
  if (!existing || existing.holder !== holder) return false;
  await fileSystem.rm(target, { force: true });
  return true;
}

/**
 * Run an operation while holding the named lock, releasing afterwards.
 */
export async function withRepoLock(name, options, operation) {
  const lock = await acquireRepoLock(name, options);
  try {
    return await operation(lock);
  } finally {
    await lock.release();
  }
}
