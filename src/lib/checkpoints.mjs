import { execFile } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
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
import { archiveRunArtifacts } from './run-artifacts.mjs';
import { acquireRepoLock } from './repo-locks.mjs';
import { parseTaskDocument } from '../planner/task.mjs';
import { parseRecipe } from './recipe.mjs';
import { parseIssueBranch } from './issue-branches.mjs';

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

async function verifiedIdentity(worktree, env, task) {
  const sources = [];
  for (const name of ['TASK.md', 'RECIPE.yml']) {
    const file = path.join(worktree, name);
    await ensureLocalPath(file, worktree);
    const entry = await fs.lstat(file);
    if (!entry.isFile() || entry.nlink !== 1 || entry.size > 65536) throw new Error('Verified checkpoint requires bounded single-link task artifacts');
    sources.push(await fs.readFile(file, 'utf8'));
  }
  const assignmentFile = path.join(worktree, 'ASSIGNMENT.md');
  await ensureLocalPath(assignmentFile, worktree);
  const assignment = await fs.lstat(assignmentFile).catch((error) => { if (error.code === 'ENOENT') return null; throw error; });
  if (assignment && (!assignment.isFile() || assignment.nlink !== 1 || assignment.size > 65536)) throw new Error('Verified checkpoint requires a bounded single-link assignment');
  sources.push(assignment ? await fs.readFile(assignmentFile, 'utf8') : null);
  parseTaskDocument(sources[0]);
  const recipe = parseRecipe(sources[1]);
  if (/^issue-[1-9]\d*$/.test(task) && recipe.ask !== `issue:${task.slice(6)}`) throw new Error('Verified checkpoint recipe does not match task identity');
  const head = (await git(worktree, ['rev-parse', '--verify', 'HEAD'], env)).trim();
  const branch = (await git(worktree, ['branch', '--show-current'], env)).trim();
  if (!oid.test(head) || !branch) throw new Error('Verified checkpoint requires a committed task branch');
  const location = await fs.realpath(worktree);
  return { head, branch, taskSource: createHash('sha256').update(JSON.stringify(sources)).digest('hex'),
    owner: createHash('sha256').update(process.platform === 'win32' ? location.toLowerCase() : location).digest('hex') };
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
        !['before-write', 'verified'].includes(record.status) || !oid.test(record.tree) ||
        record.ref !== `refs/roster/checkpoints/${task}/${record.number}` ||
        !Array.isArray(record.scope) || record.scope.some((item) => typeof item !== 'string') ||
        !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(record.time)) {
      throw new Error('Checkpoint metadata does not match the requested task');
    }
    if (record.status === 'verified' && (!record.validation || !oid.test(record.validation.head) ||
        typeof record.validation.branch !== 'string' || !record.validation.branch ||
        !/^[a-f0-9]{64}$/.test(record.validation.taskSource) || !/^[a-f0-9]{64}$/.test(record.validation.owner))) {
      throw new Error('Verified checkpoint metadata is invalid');
    }
    records.push(record);
  }
  return records;
}

export async function captureCheckpoint({ worktree, task, allowedFiles, env = process.env, apiKeyEnv, signal, verified = false, tests, expectedSnapshot }) {
  throwIfCancelled(signal);
  if (verified && (tests?.exit_code !== 0 || tests.skipped === true)) throw new Error('Verified checkpoint requires actual passing tests, not skipped evidence');
  const validation = verified ? await verifiedIdentity(worktree, env, task) : null;
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
    const products = await productFiles(worktree, allowedFiles);
    if (verified && expectedSnapshot) {
      const expected = [...expectedSnapshot.keys()].filter((file) => isAllowedFile(file, allowedFiles) && !isManagedFile(file) && !isForbiddenRead(file)).sort();
      if (JSON.stringify(expected) !== JSON.stringify([...products].sort())) throw new Error('Product set changed after verified checks');
    }
    for (const file of products) {
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
      if (verified && expectedSnapshot && expectedSnapshot.get(file) !== createHash('sha256').update(content).digest('hex')) {
        throw new Error('Product changed after verified checks; green checkpoint refused');
      }
      const hash = (await git(worktree, ['hash-object', '-w', '--no-filters', '--', file], env)).trim();
      if (verified && expectedSnapshot) {
        const blob = await git(worktree, ['cat-file', 'blob', hash], env, { encoding: 'buffer' });
        if (!blob.equals(content)) throw new Error('Product changed during verified checkpoint capture');
      }
      const mode = entry.mode & 0o111 ? '100755' : '100644';
      await git(worktree, ['update-index', '--add', '--cacheinfo', `${mode},${hash},${file}`], indexEnv);
    }
    tree = (await git(worktree, ['write-tree'], indexEnv)).trim();
    await git(worktree, ['update-ref', ref, tree, '0'.repeat(tree.length)], env);
    const record = { number, task, seat: 'coder', status: verified ? 'verified' : 'before-write', time: new Date().toISOString(),
      ref, tree, scope: allowedFiles, ...(validation ? { validation } : {}) };
    if (validation && JSON.stringify(validation) !== JSON.stringify(await verifiedIdentity(worktree, env, task))) {
      await git(worktree, ['update-ref', '-d', ref, tree], env);
      throw new Error('Task identity changed during verified checkpoint capture');
    }
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
  worktree, task, number, allowedFiles, env = process.env, apiKeyEnv, published = false,
  hasPublishedPr = branchHasPublishedPr,
  verified = false,
  signal,
}) {
  throwIfCancelled(signal);
  if (published || await hasPublishedPr({ worktree, env })) throw new Error('Rewind is refused: the worktree has a published PR.');
  throwIfCancelled(signal);
  const records = (await listCheckpoints({ worktree, task })).filter((record) => !verified || record.status === 'verified');
  const checkpoint = number === undefined ? records.at(-1) : records.find((record) => record.number === Number(number));
  if (!checkpoint) throw new Error(verified ? 'No verified green checkpoint exists for this task' : 'The requested checkpoint was not found');
  if (verified) {
    const validation = await verifiedIdentity(worktree, env, task);
    if (JSON.stringify(validation) !== JSON.stringify(checkpoint.validation) ||
        JSON.stringify(allowedFiles) !== JSON.stringify(checkpoint.scope)) {
      throw new Error('Verified checkpoint task, scope, branch, revision or worktree identity changed; restoration is refused');
    }
  }
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
  if (verified) {
    await captureCheckpoint({ worktree, task, allowedFiles, env, apiKeyEnv, signal });
    if (JSON.stringify(await verifiedIdentity(worktree, env, task)) !== JSON.stringify(checkpoint.validation)) {
      throw new Error('Verified task identity changed before restoration');
    }
  }
  throwIfCancelled(signal);
  if (verified) await archiveRunArtifacts(worktree, { task, git: (args) => git(worktree, args, env),
    preserve: ['RECIPE.yml', 'TASK.md', 'PLAN.md', 'ESTIMATE.md'] });
  throwIfCancelled(signal);
  const changed = [];
  try {
    for (const file of files) {
      throwIfCancelled(signal);
      changed.push(file);
      await replace(file, restored.get(file));
      throwIfCancelled(signal);
    }
  } catch (error) {
    try { for (const file of changed.reverse()) await replace(file, previous.get(file)); }
    catch (rollback) { throw new Error('Rewind rollback failed; inspect product files before continuing.', { cause: rollback }); }
    throw new Error('Rewind failed; prior product state was restored.', { cause: error });
  }
  return checkpoint;
}

export async function restoreVerifiedCheckpoint({ worktree, task, allowedFiles, env = process.env, signal, ...options }) {
  throwIfCancelled(signal);
  if (!/^issue-[1-9]\d*$/.test(task)) throw new Error('Verified resume requires an isolated issue task');
  const validation = await verifiedIdentity(worktree, env, task);
  const branchIdentity = parseIssueBranch(validation.branch);
  if (!branchIdentity || branchIdentity.number !== Number(task.slice(6)) ||
      branchIdentity.attempt !== null && branchIdentity.attempt > 3) {
    throw new Error('Verified resume requires the unchanged isolated task branch');
  }
  const common = (await git(worktree, ['rev-parse', '--git-common-dir'], env)).trim();
  const lock = await acquireRepoLock(task, { lockRoot: path.join(path.resolve(worktree, common), 'roster', 'wave-runs') });
  try {
    throwIfCancelled(signal);
    return await rewindCheckpoint({ ...options, worktree, task, allowedFiles, env, signal, verified: true });
  } finally { await lock.release(); }
}
