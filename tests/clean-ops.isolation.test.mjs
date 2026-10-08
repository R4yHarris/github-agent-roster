import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  ensureStateDirectory, pruneProvenance, removeStatePath, resolveCleanTarget, resolveStateRoot, runClean,
} from '../src/lib/clean-ops.mjs';
import { isContainedIn } from '../src/lib/paths.mjs';
import { acquireRepoLock } from '../src/lib/repo-locks.mjs';
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';

function fixture(t) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'clean-isolation-'));
  t.after(() => fs.rmSync(tmp, { recursive: true, force: true }));
  const root = path.join(tmp, 'state');
  fs.mkdirSync(root);
  fs.writeFileSync(path.join(root, 'keep.json'), 'sentinel');
  return { tmp, root, handle: { scope: 'worktree', root } };
}

function junction(t, target, link) {
  try {
    fs.symlinkSync(target, link, process.platform === 'win32' ? 'junction' : 'dir');
    return true;
  } catch (error) {
    if (!['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) throw error;
    t.skip(`directory links unavailable: ${error.code}`);
    return false;
  }
}

test('cleanup roots refuse every native volume root without creating or deleting entries', async () => {
  const roots = new Set([path.parse(process.cwd()).root, path.parse(os.homedir()).root]);
  if (process.platform === 'win32') roots.add('C:\\');
  for (const root of roots) {
    await assert.rejects(ensureStateDirectory({ scope: 'machine', root }),
      (error) => error.code === 'CLEAN_OPS_FORBIDDEN_PATH');
  }
});

test('Windows and POSIX containment reject siblings, other volumes and prefix collisions', () => {
  for (const [platform, root, inside, outside] of [
    ['win32', 'C:\\state', 'c:\\STATE\\runs\\one', ['C:\\state-other\\one', 'D:\\state\\one', '\\\\host\\share\\one']],
    ['linux', '/state', '/state/runs/one', ['/state-other/one', '/other/state/one', '/STATE/runs/one']],
  ]) {
    assert.equal(isContainedIn(inside, root, { platform }), true);
    for (const candidate of outside) assert.equal(isContainedIn(candidate, root, { platform }), false);
  }
});

test('scope mismatches refuse before deleting issue, repo or machine data', async (t) => {
  const { tmp } = fixture(t);
  for (const scope of ['worktree', 'repo', 'machine']) {
    const root = path.join(tmp, scope);
    fs.mkdirSync(root);
    fs.writeFileSync(path.join(root, 'record.json'), scope);
    await assert.rejects(removeStatePath({ scope, root }, 'record.json',
      { scope: scope === 'machine' ? 'repo' : 'machine' }),
    (error) => error.code === 'CLEAN_OPS_SCOPE_MISMATCH');
    assert.equal(fs.readFileSync(path.join(root, 'record.json'), 'utf8'), scope);
  }
});

test('empty or unreadable repository identity refuses target resolution without machine state', async (t) => {
  const { tmp } = fixture(t);
  const machineRoot = path.join(tmp, 'machine');
  const repoRoot = path.join(tmp, 'repo');
  fs.mkdirSync(repoRoot);
  for (const target of ['repo', 'issue']) {
    for (const run of [
      async () => ({ stdout: '' }),
      async (_command, args) => ({ stdout: args.includes('rev-parse') ? '' : 'sentinel-origin' }),
      async (_command, args) => ({ stdout: args.includes('rev-parse') ? '.git' : '' }),
      async () => { throw new Error('metadata unreadable'); },
    ]) {
      await assert.rejects(resolveCleanTarget(target, { repoRoot, machineRoot, issue: 1, run }),
        (error) => ['E_IDENTITY_INPUT', 'E_GIT_METADATA'].includes(error.code));
      assert.equal(fs.existsSync(machineRoot), false);
    }
  }
});

test('live locks block each directory scope and a retry succeeds only after release', async (t) => {
  const { tmp } = fixture(t);
  for (const scope of ['worktree', 'repo', 'machine']) {
    const root = path.join(tmp, scope);
    fs.mkdirSync(root);
    fs.writeFileSync(path.join(root, 'record.json'), scope);
    const lock = await acquireRepoLock('run', { lockRoot: root });
    try {
      await assert.rejects(runClean({ scope, root }, { execute: true, yes: true }),
        (error) => error.code === 'CLEAN_OPS_LOCK_HELD');
      assert.equal(fs.readFileSync(path.join(root, 'record.json'), 'utf8'), scope);
    } finally {
      await lock.release();
    }
    await runClean({ scope, root }, { execute: true, yes: true });
    assert.deepEqual(fs.readdirSync(root), ['locks']);
    assert.deepEqual((await runClean({ scope, root }, { execute: true, yes: true })).removed, []);
  }
});

test('a linked cleanup root is refused rather than canonicalized or created through', async (t) => {
  const { tmp, root } = fixture(t);
  const link = path.join(tmp, 'alias');
  if (!junction(t, root, link)) return;
  await assert.rejects(ensureStateDirectory({ scope: 'worktree', root: link }),
    (error) => error.name === 'ContainmentError');
  await assert.rejects(removeStatePath({ scope: 'worktree', root: link }, 'keep.json'),
    (error) => error.name === 'ContainmentError');
  assert.equal(fs.readFileSync(path.join(root, 'keep.json'), 'utf8'), 'sentinel');
  await assert.rejects(ensureStateDirectory({ scope: 'worktree', root: path.join(link, 'new-state') }),
    (error) => error.name === 'ContainmentError');
  assert.equal(fs.existsSync(path.join(root, 'new-state')), false);
});

test('provenance pruning refuses a linked store without modifying its records', async (t) => {
  const { tmp } = fixture(t);
  const root = path.join(tmp, 'provenance');
  const store = openProvenanceStore(root);
  await store.appendRecord({ id: 'raw', version: 1, section: 'raw-history' });
  const link = path.join(tmp, 'store-alias');
  if (!junction(t, root, link)) return;
  await assert.rejects(pruneProvenance(link, { target: 'machine-history', execute: true, yes: true }),
    (error) => error.name === 'ContainmentError');
  assert.deepEqual((await store.readAll()).records.map(({ id }) => id), ['raw']);
});

test('directory cleanup refuses roots overlapping source, tracked config or Git metadata', async (t) => {
  const { tmp } = fixture(t);
  const repoRoot = path.join(tmp, 'projects', 'repo');
  for (const dir of ['src', '.git', '.roster']) {
    fs.mkdirSync(path.join(repoRoot, dir), { recursive: true });
    fs.writeFileSync(path.join(repoRoot, dir, 'keep'), dir);
  }
  for (const root of [tmp, path.dirname(repoRoot), repoRoot,
    path.join(repoRoot, 'src'), path.join(repoRoot, '.git'), path.join(repoRoot, '.roster')]) {
    await assert.rejects(runClean({ scope: 'repo', root, repoRoot }, { execute: true, yes: true }),
      (error) => error.code === 'CLEAN_OPS_FORBIDDEN_PATH');
  }
  for (const dir of ['src', '.git', '.roster']) {
    assert.equal(fs.readFileSync(path.join(repoRoot, dir, 'keep'), 'utf8'), dir);
  }
});

test('a cleanup override cannot overlap its repository before it creates state', async (t) => {
  const { tmp } = fixture(t);
  const repoRoot = path.join(tmp, 'repo');
  fs.mkdirSync(repoRoot);
  const run = async (_command, args) => ({ stdout: args.includes('rev-parse') ? '.git' : 'sentinel-origin' });
  const stateRoot = path.join(repoRoot, 'src', 'generated');
  await assert.rejects(resolveStateRoot(repoRoot, { stateRoot, run }),
    (error) => error.code === 'CLEAN_OPS_FORBIDDEN_PATH');
  assert.equal(fs.existsSync(stateRoot), false);
});

test('linked entries remain untouched during partial cleanup and recovery', async (t) => {
  const { tmp, root, handle } = fixture(t);
  const otherRepo = path.join(tmp, 'other-repo');
  fs.mkdirSync(otherRepo);
  fs.writeFileSync(path.join(otherRepo, 'source.mjs'), 'preserve');
  const link = path.join(root, 'escape');
  if (!junction(t, otherRepo, link)) return;
  await assert.rejects(runClean(handle, { execute: true, yes: true }),
    (error) => error.code === 'CLEAN_OPS_PARTIAL_CLEAN' &&
      error.details.report.failed[0].code === 'CLEAN_OPS_PATH_ESCAPE');
  assert.equal(fs.readFileSync(path.join(otherRepo, 'source.mjs'), 'utf8'), 'preserve');
  assert.equal(fs.lstatSync(link).isSymbolicLink(), true);
  if (process.platform === 'win32') fs.rmdirSync(link);
  else fs.unlinkSync(link);
  assert.deepEqual((await runClean(handle, { execute: true, yes: true })).removed, []);
});
