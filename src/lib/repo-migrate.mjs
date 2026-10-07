import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { dirname, join, relative, resolve as resolvePath, sep } from 'node:path';
import { acquireRepoLock } from './repo-locks.mjs';
import {
  LEGACY_STATE_DIRNAME,
  STATE_SCOPES,
  atomicWriteFile,
  resolveStateDir,
} from './repo-state.mjs';

/**
 * Legacy -> split repo-state migration (issue #196 wave 3).
 *
 * The migration is copy-only (docs/STATE.md principle 4): it copies the
 * per-worktree state subtrees the runtime declares (checkpoints, run logs)
 * from `<root>/.roster` into the split layout `.roster-state/worktrees/<key>`
 * and never deletes, renames, or rewrites a legacy file. Everything else under
 * `.roster` — private config, fleet/capability declarations, benchmark data,
 * memory, history, and live locks — stays where it is (default-deny), so no
 * credential-bearing file ever moves.
 *
 * The migration record and backup manifest live under `.roster-state/migration`.
 */

export const MIGRATION_ID = 'split-layout-v1';
export const MIGRATION_VERSION = 1;
export const SPLIT_ROOT = '.roster-state';
export const MIGRATION_DIR = 'migration';
export const MIGRATION_RECORD = 'record.json';
export const MIGRATION_BACKUP = 'backup.json';
export const MIGRATION_LOCK = 'migrate';
export const MIGRATED_ENTRIES = Object.freeze(['checkpoints', 'runs']);
const TEMP_SUFFIX = '.migrate-tmp';

export const MIGRATION_ERROR_CODES = Object.freeze({
  E_ACTIVE_RUN: 'E_ACTIVE_RUN',
  E_CORRUPT_STATE: 'E_CORRUPT_STATE',
  E_CONFLICT: 'E_CONFLICT',
  E_ROLLBACK: 'E_ROLLBACK',
  E_ROLLBACK_FAILED: 'E_ROLLBACK_FAILED',
});

export class MigrationError extends Error {
  constructor(message, { code, path, cause } = {}) {
    super(message);
    this.name = 'MigrationError';
    if (code !== undefined) this.code = code;
    if (path !== undefined) this.path = path;
    if (cause !== undefined) this.cause = cause;
  }
}

const defaultIsAlive = (pid) => {
  if (!Number.isSafeInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === 'EPERM';
  }
};

function toPosix(path) {
  return path.split(sep).join('/');
}

function sha256(buffer) {
  return createHash('sha256').update(buffer).digest('hex');
}

async function exists(path, fileSystem) {
  try {
    await fileSystem.stat(path);
    return true;
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ENOTDIR') return false;
    throw error;
  }
}

async function listFiles(start, fileSystem) {
  const files = [];
  let stat;
  try {
    stat = await fileSystem.stat(start);
  } catch (error) {
    if (error.code === 'ENOENT') return files;
    throw error;
  }
  if (stat.isFile()) return [start];
  const stack = [start];
  while (stack.length > 0) {
    const dir = stack.pop();
    for (const entry of await fileSystem.readdir(dir, { withFileTypes: true })) {
      const absolute = join(dir, entry.name);
      if (entry.isDirectory()) stack.push(absolute);
      else if (entry.isFile()) files.push(absolute);
    }
  }
  return files.sort();
}

/** Resolve the source and destination layout for a repository root. */
export function migrationLayout(root) {
  const repoRoot = resolvePath(root);
  const destination = resolveStateDir({
    scope: STATE_SCOPES.PER_WORKTREE, repoRoot, worktreeRoot: repoRoot,
  }).dir;
  const migrationDir = join(repoRoot, SPLIT_ROOT, MIGRATION_DIR);
  return {
    root: repoRoot,
    source: join(repoRoot, LEGACY_STATE_DIRNAME),
    destination,
    sourceLayout: LEGACY_STATE_DIRNAME,
    destinationLayout: toPosix(relative(repoRoot, destination)),
    record: join(migrationDir, MIGRATION_RECORD),
    backup: join(migrationDir, MIGRATION_BACKUP),
  };
}

/**
 * Detect active runs from the repo-lock records written by `acquireRepoLock`
 * in both the legacy and split lock directories. A lock whose holder pid is
 * alive is active; an unreadable lock record is treated as active so the
 * migration fails closed instead of guessing.
 */
export async function detectActiveRun(root, { fileSystem = fs, isAlive = defaultIsAlive } = {}) {
  const layout = migrationLayout(root);
  const locks = [];
  for (const dir of [join(layout.source, 'locks'), join(layout.destination, 'locks')]) {
    let entries;
    try {
      entries = await fileSystem.readdir(dir);
    } catch (error) {
      if (error.code === 'ENOENT') continue;
      throw error;
    }
    for (const name of entries.filter((entry) => entry.endsWith('.lock')).sort()) {
      const path = join(dir, name);
      let record = null;
      try {
        record = JSON.parse(await fileSystem.readFile(path, 'utf8'));
      } catch {
        record = null;
      }
      const pid = record && Number.isSafeInteger(record.pid) ? record.pid : null;
      if (record === null || (pid !== null && isAlive(pid))) {
        locks.push({ path, holder: record?.holder ?? 'unreadable lock record', pid });
      }
    }
  }
  return { active: locks.length > 0, locks };
}

/** Read the migration record; corrupt content fails closed with recovery steps. */
export async function readMigrationRecord(root, { fileSystem = fs } = {}) {
  const { record, backup } = migrationLayout(root);
  let text;
  try {
    text = await fileSystem.readFile(record, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
  let value;
  try {
    value = JSON.parse(text);
  } catch (error) {
    throw corrupt(record, backup, 'is not valid JSON', error);
  }
  const valid = value && value.id === MIGRATION_ID && Number.isSafeInteger(value.version) &&
    ['in-progress', 'complete'].includes(value.status) && Array.isArray(value.files) &&
    value.files.every((file) => typeof file?.path === 'string' && /^[0-9a-f]{64}$/.test(file.sha256 ?? ''));
  if (!valid) throw corrupt(record, backup, 'does not match the split-layout-v1 record shape');
  return value;
}

function corrupt(record, backup, reason, cause) {
  return new MigrationError(
    `Migration record ${record} ${reason}. Nothing was migrated or deleted; the legacy ` +
    `${LEGACY_STATE_DIRNAME} state is unchanged. Recovery: compare the split layout with ${backup} ` +
    `(the pre-migration manifest), restore or remove ${record}, then re-run the migration with ` +
    'dryRun first.',
    { code: MIGRATION_ERROR_CODES.E_CORRUPT_STATE, path: record, cause });
}

async function readSource(path, fileSystem) {
  try {
    return await fileSystem.readFile(path);
  } catch (error) {
    throw new MigrationError(
      `Legacy state file ${path} could not be read (${error.code ?? error.message}). Recovery: ` +
      'repair or remove that file, then re-run the migration with dryRun first.',
      { code: MIGRATION_ERROR_CODES.E_CORRUPT_STATE, path, cause: error });
  }
}

async function plan(layout, previous, fileSystem) {
  // Files this migration wrote before: an in-progress record owns any content
  // (a partial copy); a complete record owns the bytes it recorded, so a copy
  // that still matches is refreshed from the legacy source while anything a
  // person changed since is never overwritten.
  const owned = new Map((previous?.files ?? []).map((file) => [file.path, file.sha256]));
  const ownsContent = (rel, current) => owned.has(rel) &&
    (previous.status === 'in-progress' || owned.get(rel) === sha256(current));
  const copies = [];
  const repairs = [];
  const temporaries = [];
  let already = 0;
  for (const entry of MIGRATED_ENTRIES) {
    const destinationEntry = join(layout.destination, entry);
    for (const file of await listFiles(destinationEntry, fileSystem)) {
      if (file.endsWith(TEMP_SUFFIX)) temporaries.push(file);
    }
    for (const from of await listFiles(join(layout.source, entry), fileSystem)) {
      const rel = toPosix(relative(layout.source, from));
      const to = join(layout.destination, rel);
      const bytes = await readSource(from, fileSystem);
      const hash = sha256(bytes);
      let current = null;
      try {
        current = await fileSystem.readFile(to);
      } catch (error) {
        if (error.code !== 'ENOENT') throw error;
      }
      if (current !== null && sha256(current) === hash) {
        already += 1;
        continue;
      }
      const move = { path: rel, from, to, sha256: hash, bytes: bytes.length };
      if (current === null) {
        copies.push(move);
      } else if (ownsContent(rel, current)) {
        // The legacy source is intact, so a destination this migration wrote
        // is rewritten from it; its prior bytes are kept for rollback.
        repairs.push({ ...move, previous: current });
      } else {
        throw new MigrationError(
          `Split-layout file ${to} already exists with different content than ${from}. The ` +
          'migration never overwrites state it did not write. Recovery: decide which copy is ' +
          'current, move the other aside, then re-run the migration with dryRun first.',
          { code: MIGRATION_ERROR_CODES.E_CONFLICT, path: to });
      }
    }
  }
  return { copies, repairs, temporaries, already };
}

async function retainedEntries(layout, fileSystem) {
  try {
    return (await fileSystem.readdir(layout.source))
      .filter((name) => !MIGRATED_ENTRIES.includes(name))
      .sort();
  } catch (error) {
    if (error.code === 'ENOENT') return [];
    throw error;
  }
}

async function writeJson(file, value, fileSystem) {
  await fileSystem.mkdir(dirname(file), { recursive: true, mode: 0o700 });
  await atomicWriteFile(file, `${JSON.stringify(value, null, 2)}\n`, { fileSystem });
}

async function copyOne(move, fileSystem) {
  await fileSystem.mkdir(dirname(move.to), { recursive: true, mode: 0o700 });
  const temporary = `${move.to}${TEMP_SUFFIX}`;
  await fileSystem.copyFile(move.from, temporary);
  const copied = await fileSystem.readFile(temporary);
  if (sha256(copied) !== move.sha256) {
    throw new MigrationError(`Copy of ${move.from} to ${move.to} did not verify.`,
      { code: MIGRATION_ERROR_CODES.E_CORRUPT_STATE, path: move.to });
  }
  await fileSystem.rename(temporary, move.to);
}

async function removeEmptyParents(file, stop, fileSystem) {
  let dir = dirname(file);
  while (dir.startsWith(stop) && dir !== stop) {
    try {
      await fileSystem.rmdir(dir);
    } catch {
      return;
    }
    dir = dirname(dir);
  }
}

async function rollback(layout, touched, previousRecord, fileSystem) {
  const failures = [];
  for (const move of [...touched].reverse()) {
    try {
      await fileSystem.rm(`${move.to}${TEMP_SUFFIX}`, { force: true });
      if (move.previous === undefined) {
        await fileSystem.rm(move.to, { force: true });
        await removeEmptyParents(move.to, join(layout.root, SPLIT_ROOT), fileSystem);
      } else {
        await fileSystem.writeFile(move.to, move.previous);
      }
    } catch (error) {
      failures.push(`${move.to}: ${error.code ?? error.message}`);
    }
  }
  try {
    if (previousRecord === null) await fileSystem.rm(layout.record, { force: true });
    else await fileSystem.writeFile(layout.record, previousRecord);
  } catch (error) {
    failures.push(`${layout.record}: ${error.code ?? error.message}`);
  }
  return failures;
}

/**
 * Migrate legacy `.roster` per-worktree state into the split layout.
 *
 * `dryRun` reports the plan (counts, layout names, active-run state, retained
 * legacy entries) without touching the filesystem. A real run detects active
 * runs first, holds the `migrate` repo lock, writes the backup manifest and an
 * in-progress record before copying, and rolls every created file back if a
 * copy fails. A second run plans zero changes and leaves the tree identical.
 */
export async function migrate(root, {
  dryRun = false,
  fileSystem = fs,
  isAlive = defaultIsAlive,
  now = () => new Date().toISOString(),
} = {}) {
  const layout = migrationLayout(root);
  const activeRun = await detectActiveRun(layout.root, { fileSystem, isAlive });
  const previous = await readMigrationRecord(layout.root, { fileSystem });
  const planned = await plan(layout, previous, fileSystem);
  const moves = [...planned.copies, ...planned.repairs];
  const report = {
    id: MIGRATION_ID,
    dryRun,
    applied: false,
    sourceLayout: layout.sourceLayout,
    destinationLayout: layout.destinationLayout,
    activeRun,
    interrupted: previous?.status === 'in-progress',
    planned: {
      files: moves.length,
      bytes: moves.reduce((sum, move) => sum + move.bytes, 0),
      repairs: planned.repairs.length,
      temporaries: planned.temporaries.length,
    },
    alreadyMigrated: planned.already,
    retained: await retainedEntries(layout, fileSystem),
    moves: moves.map((move) => ({
      from: toPosix(relative(layout.root, move.from)),
      to: toPosix(relative(layout.root, move.to)),
    })),
    record: toPosix(relative(layout.root, layout.record)),
  };
  if (dryRun) return report;
  if (activeRun.active) {
    throw new MigrationError(
      `Active run detected in ${layout.root} (${activeRun.locks.map((lock) => `${lock.holder} at ${lock.path}`).join('; ')}). ` +
      'Migration refused before touching any file so the active run is preserved. Recovery: let the ' +
      'run finish (or remove a lock whose holder has exited), then re-run the migration with dryRun first.',
      { code: MIGRATION_ERROR_CODES.E_ACTIVE_RUN, path: activeRun.locks[0].path });
  }
  // Nothing to copy and no interrupted run to finish: write nothing at all, so
  // a second run (or a repo without legacy state) leaves the tree identical.
  if (moves.length === 0 && planned.temporaries.length === 0 && previous?.status !== 'in-progress') {
    return report;
  }

  const lock = await acquireRepoLock(MIGRATION_LOCK, { repoRoot: layout.root, worktreeRoot: layout.root, fileSystem });
  const touched = [];
  try {
    for (const temporary of planned.temporaries) await fileSystem.rm(temporary, { force: true });
    const recorded = new Map((previous?.files ?? []).map((file) => [file.path, file.sha256]));
    for (const move of moves) recorded.set(move.path, move.sha256);
    const files = [...recorded].sort(([a], [b]) => a.localeCompare(b))
      .map(([path, hash]) => ({ path, sha256: hash }));
    const previousRecord = previous === null ? null : await fileSystem.readFile(layout.record);
    if (!await exists(layout.backup, fileSystem)) {
      const prior = [];
      for (const entry of MIGRATED_ENTRIES) {
        for (const file of await listFiles(join(layout.destination, entry), fileSystem)) {
          prior.push({ path: toPosix(relative(layout.root, file)), sha256: sha256(await fileSystem.readFile(file)) });
        }
      }
      await writeJson(layout.backup, {
        id: MIGRATION_ID, createdAt: now(), sourceLayout: layout.sourceLayout,
        destinationLayout: layout.destinationLayout, legacyRetained: true, prior,
      }, fileSystem);
    }
    const record = {
      id: MIGRATION_ID, version: MIGRATION_VERSION, sourceLayout: layout.sourceLayout,
      destinationLayout: layout.destinationLayout, legacyRetained: true, files,
    };
    await writeJson(layout.record, { ...record, status: 'in-progress' }, fileSystem);
    try {
      for (const move of moves) {
        touched.push(move);
        await copyOne(move, fileSystem);
      }
    } catch (error) {
      const failures = await rollback(layout, touched, previousRecord, fileSystem);
      if (failures.length > 0) {
        throw new MigrationError(
          `Migration of ${layout.root} failed (${error.message}) and rollback could not remove ` +
          `${failures.join(', ')}. The legacy ${LEGACY_STATE_DIRNAME} state is unchanged. Recovery: ` +
          `remove those files by hand, compare with ${layout.backup}, then re-run the migration.`,
          { code: MIGRATION_ERROR_CODES.E_ROLLBACK_FAILED, path: layout.root, cause: error });
      }
      throw new MigrationError(
        `Migration of ${layout.root} failed (${error.message}); every file it created was rolled back ` +
        `and the legacy ${LEGACY_STATE_DIRNAME} state is unchanged. Recovery: fix the cause, then ` +
        `re-run the migration with dryRun first; ${layout.backup} keeps the pre-migration manifest.`,
        { code: MIGRATION_ERROR_CODES.E_ROLLBACK, path: layout.root, cause: error });
    }
    await writeJson(layout.record, { ...record, status: 'complete' }, fileSystem);
  } finally {
    await lock.release();
  }
  return { ...report, applied: true };
}
