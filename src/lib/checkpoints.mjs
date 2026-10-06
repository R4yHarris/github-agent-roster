import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { githubRepository } from './issue.mjs';
import { ensureLocalPath } from './paths.mjs';
import { STATE_SCOPES, statePaths, managedIgnorePrefixes, checkpointIgnorePattern, LEGACY_STATE_DIRNAME } from './repo-state.mjs';
import { isAllowedFile, isForbiddenRead, isManagedFile } from '../runtime/tools.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';
import { throwIfCancelled } from '../runtime/cancel.mjs';
import { ensureManagedIgnored } from './managed.mjs';

const execute = promisify(execFile);
const oid = /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/;

function identity(task) {
  if (typeof task !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(task)) {
    throw new TypeError('Checkpoint task must be an opaque identifier');
  }
  return task.startsWith('issue-') ? task.slice(6) : task;
}

async function git(worktree, args, env = process.env, options = {}) {
  try {
    return (await execute('git', ['--literal-pathspecs', ...args], {
      cwd: worktree, env, encoding: 'utf8', timeout: 30000, maxBuffer: 16 * 1024 * 1024, ...options,
    })).stdout;
  } catch (error) {
    throw new Error('Checkpoint unavailable; no product restoration was authorized.', { cause: error });
  }
}

async function productFiles(worktree, allowedFiles) {
  const files = [];
  const managedPrefixes = managedIgnorePrefixes(worktree);
  async function visit(directory = '') {
    for (const entry of await fs.readdir(path.join(worktree, directory), { withFileTypes: true })) {
      const file = path.posix.join(directory, entry.name);
      if (isForbiddenRead(file) || isManagedFile(file) ||
          managedPrefixes.some((prefix) => file.startsWith(prefix)) ||
          ['node_modules', '.worktrees'].includes(file.split('/')[0])) continue;
      if (entry.isSymbolicLink()) {
        if (isAllowedFile(file, allowedFiles)) throw new Error('Checkpoint product paths may not be symlinks');
      } else if (entry.isDirectory()) await visit(file);
      else if (entry.isFile() && isAllowedFile(file, allowedFiles)) {
        if (files.length === 2048) throw new Error('Checkpoint product scope exceeds 2048 files');
        files.push(file);
      }
    }
  }
  await visit();
  return files.sort();
}

// Checkpoints are PER_WORKTREE state: each linked worktree owns its own
// checkpoint tree, resolved through the repo-state API off worktree identity.
// The legacy pre-split layout stays the on-disk truth for per-worktree state;
// the repo-state API owns the path construction, scoping, and legacy-layout
// detection so no consumer spells the private directory name itself.
async function checkpointDir(worktree, task) {
  const resolved = await statePaths({
    scope: STATE_SCOPES.PER_WORKTREE,
    repoRoot: worktree,
    worktreeRoot: worktree,
    layoutDirName: LEGACY_STATE_DIRNAME,
    segments: ['checkpoints', identity(task)],
  });
  return resolved.path;
}

export async function listCheckpoints({ worktree, task }) {
  const directory = await checkpointDir(worktree, task);
  await ensureLocalPath(directory, worktree);
  const entries = await fs.readdir(directory).catch((error) => {
    if (error.code === 'ENOENT') return [];
    throw error;
  });
  const records = [];
  for (const name of entries.filter((entry) => /^[1-9]\d*$/.test(entry)).sort((a, b) => Number(a) - Number(b))) {
    const file = path.join(directory, name);
    await ensureLocalPath(file, worktree);
    const status = await fs.lstat(file);
    if (!status.isFile() || status.isSymbolicLink() || status.nlink !== 1 || status.size > 262144) {
      throw new Error('Checkpoint metadata must be a bounded regular single-link file');
    }
    let record;
    try { record = JSON.parse(await fs.readFile(file, 'utf8')); }
    catch { throw new Error('Checkpoint metadata is invalid'); }
    if (!Number.isSafeInteger(record.number) || record.number < 1 || record.number !== Number(name) || record.task !== task || record.seat !== 'coder' ||
        record.status !== 'before-write' || !oid.test(record.tree) ||
        record.ref !== `refs/roster/checkpoints/${task}/${record.number}` ||
        !Array.isArray(record.scope) || record.scope.some((item) => typeof item !== 'string') ||
        !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(record.time)) {
      throw new Error('Checkpoint metadata does not match the requested task');
    }
    records.push(record);
  }
  return records;
}

export async function captureCheckpoint({ worktree, task, allowedFiles, env = process.env, apiKeyEnv, signal }) {
  throwIfCancelled(signal);
  if (redactEvidence(task, { env, apiKeyEnv }) !== task) throw new Error('Checkpoint identifiers must not contain secret material');
  if (!Array.isArray(allowedFiles) || allowedFiles.some((file) =>
    typeof file !== 'string' || redactEvidence(file, { env, apiKeyEnv }) !== file)) {
    throw new Error('Checkpoint scope must contain public task paths, not secret material');
  }
  const records = await listCheckpoints({ worktree, task });
  const number = (records.at(-1)?.number ?? 0) + 1;
  const directory = await checkpointDir(worktree, task);
  await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  await ensureLocalPath(directory, worktree);
  await ensureManagedIgnored(worktree, checkpointIgnorePattern(), env);
  const index = path.join(directory, `index-${randomBytes(8).toString('hex')}`);
  const indexEnv = { ...env, GIT_INDEX_FILE: index };
  const ref = `refs/roster/checkpoints/${task}/${number}`;
  let tree;
  try {
    await git(worktree, ['read-tree', '--empty'], indexEnv);
    for (const file of await productFiles(worktree, allowedFiles)) {
      throwIfCancelled(signal);
      const target = path.join(worktree, file);
      await ensureLocalPath(target, worktree);
      const entry = await fs.lstat(target);
      if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1 || entry.size > 16 * 1024 * 1024) {
        throw new Error('Checkpoint product must be a regular single-link file of at most 16 MiB');
      }
      const content = await fs.readFile(target);
      if (redactEvidence(content.toString('utf8'), { env, apiKeyEnv }) !== content.toString('utf8')) {
        throw new Error('Checkpoint refuses product files containing secret material');
      }
      const hash = (await git(worktree, ['hash-object', '-w', '--no-filters', '--', file], env)).trim();
      const mode = entry.mode & 0o111 ? '100755' : '100644';
      await git(worktree, ['update-index', '--add', '--cacheinfo', `${mode},${hash},${file}`], indexEnv);
    }
    tree = (await git(worktree, ['write-tree'], indexEnv)).trim();
    await git(worktree, ['update-ref', ref, tree, '0'.repeat(tree.length)], env);
    const record = { number, task, seat: 'coder', status: 'before-write', time: new Date().toISOString(),
      ref, tree, scope: allowedFiles };
    try {
      await fs.writeFile(path.join(directory, String(number)), `${JSON.stringify(record)}\n`,
        { encoding: 'utf8', flag: 'wx', mode: 0o600 });
    } catch (error) {
      await git(worktree, ['update-ref', '-d', ref, tree], env);
      throw error;
    }
    return record;
  } finally {
    await fs.unlink(index).catch((error) => { if (error.code !== 'ENOENT') throw error; });
  }
}

export async function branchHasPublishedPr({ worktree, env = process.env, runCommand } = {}) {
  const run = runCommand ?? (async (program, args) => {
    try {
      return (await execute(program, args, { cwd: worktree, env, encoding: 'utf8', timeout: 30000 })).stdout;
    } catch (error) { throw new Error('Could not verify published PRs; rewind is refused.', { cause: error }); }
  });
  const branch = (await run('git', ['branch', '--show-current'])).trim();
  if (!branch) throw new Error('Rewind requires a checked-out task branch');
  const repository = githubRepository((await run('git', ['remote', 'get-url', 'origin'])).trim());
  let pulls;
  try { pulls = JSON.parse(await run('gh', ['pr', 'list', '--repo', repository, '--head', branch,
    '--state', 'all', '--limit', '1', '--json', 'number'])); }
  catch (error) { throw new Error('Could not verify published PRs; rewind is refused.', { cause: error }); }
  if (!Array.isArray(pulls) || pulls.some((pull) => !Number.isSafeInteger(pull?.number) || pull.number < 1)) {
    throw new Error('Published PR metadata is invalid; rewind is refused.');
  }
  return pulls.length > 0;
}

export async function rewindCheckpoint({
  worktree, task, number, allowedFiles, env = process.env, published = false,
  hasPublishedPr = branchHasPublishedPr,
}) {
  if (published || await hasPublishedPr({ worktree, env })) throw new Error('Rewind is refused: the worktree has a published PR.');
  const records = await listCheckpoints({ worktree, task });
  const checkpoint = number === undefined ? records.at(-1) : records.find((record) => record.number === Number(number));
  if (!checkpoint) throw new Error('The requested checkpoint was not found');
  if ((await git(worktree, ['rev-parse', '--verify', checkpoint.ref], env)).trim() !== checkpoint.tree) {
    throw new Error('Checkpoint ref changed; restoration is refused');
  }
  const entries = (await git(worktree, ['ls-tree', '-r', '-z', checkpoint.tree], env)).split('\0').filter(Boolean);
  const restored = new Map();
  for (const entry of entries) {
    const match = /^(100644|100755) blob ([a-f0-9]+)\t(.+)$/.exec(entry);
    if (!match || !isAllowedFile(match[3], allowedFiles) || !isAllowedFile(match[3], checkpoint.scope)) {
      throw new Error('Checkpoint paths exceed the current task product scope');
    }
    const content = await git(worktree, ['cat-file', 'blob', match[2]], env, { encoding: 'buffer' });
    restored.set(match[3], { content, mode: match[1] });
  }
  const current = await productFiles(worktree, checkpoint.scope);
  const files = [...new Set([...current, ...restored.keys()])];
  const previous = new Map();
  for (const file of files) {
    if (!isAllowedFile(file, allowedFiles)) throw new Error('Current task scope changed; rewind is refused');
    const target = path.join(worktree, file);
    await ensureLocalPath(target, worktree);
    const entry = await fs.lstat(target).catch((error) => { if (error.code === 'ENOENT') return null; throw error; });
    if (entry && (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1)) {
      throw new Error('Rewind product paths must remain regular single-link files');
    }
    previous.set(file, entry ? { content: await fs.readFile(target), mode: entry.mode & 0o111 ? '100755' : '100644' } : null);
  }
  const replace = async (file, saved) => {
    const target = path.join(worktree, file);
    if (!saved) await fs.unlink(target).catch((error) => { if (error.code !== 'ENOENT') throw error; });
    else {
      await fs.mkdir(path.dirname(target), { recursive: true });
      const handle = await fs.open(target, constants.O_WRONLY | constants.O_CREAT | constants.O_TRUNC |
        (constants.O_NOFOLLOW ?? 0), saved.mode === '100755' ? 0o755 : 0o644);
      try { await handle.writeFile(saved.content); }
      finally { await handle.close(); }
    }
  };
  const changed = [];
  try {
    for (const file of files) {
      changed.push(file);
      await replace(file, restored.get(file));
    }
  } catch (error) {
    try { for (const file of changed.reverse()) await replace(file, previous.get(file)); }
    catch (rollback) { throw new Error('Rewind rollback failed; inspect product files before continuing.', { cause: rollback }); }
    throw new Error('Rewind failed; prior product state was restored.', { cause: error });
  }
  return checkpoint;
}
