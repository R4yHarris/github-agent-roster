import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readdir, readFile, rm, writeFile, stat } from 'node:fs/promises';
import path from 'node:path';
import { tmpdir } from 'node:os';
import { execFile, execFileSync } from 'node:child_process';
import { promisify } from 'node:util';
import { resolveRepoIdentity } from '../src/lib/repo-identity.mjs';
import { openProvenanceStore, LOG_DIRNAME, SEGMENT_NAME } from '../src/lib/provenance-store.mjs';
import { createRecord } from '../src/lib/provenance-schema.mjs';
import { createProvenanceStore } from '../src/lib/provenance-api.mjs';
import { identityHash } from '../src/lib/repo-identity.mjs';
import { createHistoryReader, filterRecords, filterRepositories } from '../src/lib/history-query.mjs';
import { runHistory, loadProvenanceRecords, resolveHistoryRoot, parseListFlags, formatList } from '../src/lib/history-cli.mjs';

const execute = promisify(execFile);
const REPO_A = identityHash({ gitCommonDir: 'common-a', remoteUrl: 'sentinel-a' });
const REPO_B = identityHash({ gitCommonDir: 'common-b', remoteUrl: 'sentinel-b' });
const JAN = Date.parse('2024-01-01T00:00:00Z');
const FEB = Date.parse('2024-02-01T00:00:00Z');
const MAR = Date.parse('2024-03-01T00:00:00Z');

async function fixture(t, { populate = true } = {}) {
  // Keep isolated scratch directories inside the worktree, not OS temp.
  const dir = await mkdtemp(path.join(process.cwd(), '.history-test-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const root = path.join(dir, 'store');
  const store = openProvenanceStore(root);
  if (populate) {
    await store.appendRecord({
      ...createRecord({ run: 'r1', session: 's1', issue: { issue: '10' }, seat: { name: 'coder' },
        servedModel: 'served-a', requestedModel: 'requested-a' }, JAN),
      id: 'record-1', version: 1, repoIdentity: REPO_A, event: 'completed', section: 'raw-history',
      payload: { outcome: 'success' },
    });
    await store.appendRecord({
      ...createRecord({ run: 'r2', session: 's2' }, FEB),
      id: 'record-2', version: 1, repoIdentity: REPO_B, event: 'failure', section: 'raw-history',
      payload: { issue: 20, seat: 'reviewer', model: 'payload-model', outcome: 'rework' },
    });
    await store.appendRecord({
      ...createRecord({ run: 'r3', session: 's3', issue: { issue: '10' }, seat: { name: 'planner' },
        requestedModel: 'requested-a' }, MAR),
      id: 'record-3', version: 1, repoIdentity: REPO_A, event: 'completed', section: 'raw-history', payload: {},
    });
  }
  return { dir, root, store, reader: createHistoryReader({ root }) };
}

const ids = (rows) => rows.map((row) => row.runId).sort();

async function snapshot(root) {
  const entries = [];
  async function walk(dir, relative = '') {
    for (const entry of (await readdir(dir, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
      const name = path.join(relative, entry.name);
      const file = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        entries.push([name, 'directory']);
        await walk(file, name);
      } else entries.push([name, (await readFile(file)).toString('base64')]);
    }
  }
  await walk(root);
  return entries;
}

test('real durable writer records list across stable repo identities independently and together', async (t) => {
  const { reader } = await fixture(t);
  const all = await reader.list();
  assert.deepEqual(ids(all), ['r1', 'r2', 'r3']);
  assert.deepEqual(ids(await reader.list({ repository: REPO_A })), ['r1', 'r3']);
  assert.deepEqual(ids(await reader.list({ repository: REPO_B })), ['r2']);
  assert.deepEqual(ids(filterRepositories(all, [REPO_A, REPO_B])), ['r1', 'r2', 'r3']);
  assert.deepEqual(ids(filterRepositories(all, REPO_B)), ['r2']);
  assert.deepEqual(await reader.list({ repository: 'common-a' }), []);
  assert.deepEqual(filterRecords([{ repository: { hash: REPO_A } }], { repository: REPO_A }), []);
});

test('filters schema fields and typed API payload fallbacks with empty schema defaults', async (t) => {
  const { reader } = await fixture(t);
  for (const [filters, expected] of [
    [{ issue: '10' }, ['r1', 'r3']], [{ issue: 20 }, ['r2']],
    [{ seat: 'coder' }, ['r1']], [{ seat: 'reviewer' }, ['r2']],
    [{ model: 'served-a' }, ['r1']], [{ model: 'requested-a' }, ['r3']],
    [{ model: 'payload-model' }, ['r2']], [{ outcome: 'success' }, ['r1']],
    [{ outcome: 'rework' }, ['r2']], [{ outcome: 'completed' }, ['r3']],
    [{ repository: REPO_A, issue: '10', seat: 'coder', model: 'served-a', outcome: 'success' }, ['r1']],
  ]) assert.deepEqual(ids(await reader.list(filters)), expected);
});

test('createdAt time windows are inclusive for epoch and ISO bounds', async (t) => {
  const { reader } = await fixture(t);
  assert.deepEqual(ids(await reader.list({ since: FEB })), ['r2', 'r3']);
  assert.deepEqual(ids(await reader.list({ until: '2024-02-01T00:00:00Z' })), ['r1', 'r2']);
  assert.deepEqual(ids(await reader.list({ since: FEB, until: FEB })), ['r2']);
  assert.deepEqual(filterRecords([{ createdAt: null }], { since: JAN }), []);
});

test('unknown filters, invalid times, and reversed time ranges are rejected even on empty stores', () => {
  assert.throws(() => filterRecords([], { bogus: 1 }), /Unknown history filter/);
  assert.throws(() => filterRecords([], { since: 'invalid' }), /Invalid history since/);
  assert.throws(() => filterRecords([], { until: Infinity }), /Invalid history until/);
  assert.throws(() => filterRecords([], { since: MAR, until: JAN }), /since must not be after until/);
});

test('show matches stable run, session, and record ids', async (t) => {
  const { reader } = await fixture(t);
  for (const id of ['r2', 's2', 'record-2']) assert.equal((await reader.show(id)).runId, 'r2');
  await assert.rejects(reader.show('missing'), /No history record found/);
  await assert.rejects(reader.show(''), /requires a session/);
});

test('list and show preserve every store name and byte, including corrupt and interrupted entries', async (t) => {
  const { dir, root, reader } = await fixture(t);
  await writeFile(path.join(root, LOG_DIRNAME, 'bad.json'), '{broken');
  await writeFile(path.join(root, LOG_DIRNAME, 'future.json'), JSON.stringify({ id: 'future', version: 999 }));
  await writeFile(path.join(root, LOG_DIRNAME, 'mismatch.json'), JSON.stringify({ id: 'other', version: 1 }));
  await writeFile(path.join(root, LOG_DIRNAME, '.interrupted.nonce.tmp'), 'partial write');
  await writeFile(path.join(root, `${SEGMENT_NAME}.nonce.tmp`), 'interrupted segment');
  await writeFile(path.join(root, SEGMENT_NAME), 'torn segment');
  const before = await snapshot(dir);
  assert.equal((await reader.list()).length, 3);
  assert.equal((await reader.show('s1')).runId, 'r1');
  const list = await runHistory(['list', '--store', root]);
  assert.match(list, /Skipped 3/);
  assert.match(list, /session=s1.*run=r1/);
  assert.match(await runHistory(['show', 'record-2', '--store', root]), /session=s2.*run=r2/);
  await assert.rejects(runHistory(['show', 'missing', '--store', root]), /No history record found/);
  assert.deepEqual(await snapshot(dir), before);
});

test('missing store is empty, clearly reported, and never created', async (t) => {
  const { dir, root, reader } = await fixture(t, { populate: false });
  assert.deepEqual(await reader.list(), []);
  assert.deepEqual(await loadProvenanceRecords({ storePath: root }), { records: [], skipped: [], root, missing: true });
  assert.match(await runHistory(['list', '--store', root]), /No history store found/);
  assert.deepEqual(await snapshot(dir), []);
  await assert.rejects(stat(root), { code: 'ENOENT' });
});

test('never scans guessed repository-private JSON files', async (t) => {
  const { dir, root } = await fixture(t, { populate: false });
  await mkdir(path.join(dir, 'provenance'));
  await mkdir(path.join(dir, 'history'));
  await mkdir(path.join(dir, 'roster'));
  for (const name of ['provenance.json', path.join('provenance', 'history.json'),
    path.join('history', 'provenance.json'), path.join('roster', 'history.json')]) {
    await writeFile(path.join(dir, name), JSON.stringify([{ runId: 'private-sentinel' }]));
  }
  const before = await snapshot(dir);
  const output = await runHistory(['list'], { cwd: dir, run: async () => ({ stdout: '.git\n' }) });
  assert.match(output, /No history store found/);
  assert.doesNotMatch(output, /private-sentinel/);
  assert.deepEqual(await snapshot(dir), before);
  assert.equal((await loadProvenanceRecords({ cwd: dir, storePath: root })).records.length, 0);
});

test('resolves common git directory from main, linked, and nested checkouts via injected git seam', async (t) => {
  const { dir } = await fixture(t, { populate: false });
  const main = path.join(dir, 'main');
  const common = path.join(main, '.git');
  for (const [cwd, stdout] of [[main, '.git'], [path.join(dir, 'linked'), common],
    [path.join(main, 'nested'), path.join('..', '.git')]]) {
    const run = async (command, args, options) => {
      assert.equal(command, 'git');
      assert.deepEqual(args, ['--no-pager', 'rev-parse', '--git-common-dir']);
      assert.equal(options.cwd, cwd);
      return { stdout: `${stdout}\n` };
    };
    assert.equal(await resolveHistoryRoot({ cwd, run }), path.join(common, 'roster', 'provenance'));
  }
});

test('default list reads the same common-directory store without remote access', async (t) => {
  const { dir } = await fixture(t, { populate: false });
  const common = path.join(dir, '.git');
  const root = path.join(common, 'roster', 'provenance');
  await openProvenanceStore(root).appendRecord({ id: 'default', version: 1, runId: 'default-run', repoIdentity: REPO_A });
  assert.match(await runHistory(['list'], { cwd: dir, run: async () => ({ stdout: common }) }), /run=default-run/);
});

test('explicit relative --store bypasses git resolution for list and show', async (t) => {
  const { dir } = await fixture(t);
  const options = { cwd: dir, run: () => { throw new Error('git must not be called'); } };
  assert.match(await runHistory(['list', '--store', 'store', '--repo', REPO_B], options), /run=r2/);
  assert.match(await runHistory(['show', 's1', '--store', 'store'], options), /run=r1/);
});

test('typed ProvenanceStore writer output is queryable through the real durable reader', async (t) => {
  const { dir, root, reader } = await fixture(t, { populate: false });
  const run = async (_command, args) => ({ stdout: args.includes('rev-parse') ? '.git\n' : 'sentinel-origin\n' });
  const writer = createProvenanceStore({ root, repoRoot: dir, run });
  const saved = await writer.recordEvent({ runId: 'typed-run', sessionId: 'typed-session', event: 'completed',
    payload: { issue: 284, seat: 'coder', model: 'typed-model', outcome: 'success' } });
  assert.equal(saved.section, 'raw-history');
  assert.deepEqual(await reader.list({ repository: await writer.identity(), issue: 284, seat: 'coder',
    model: 'typed-model', outcome: 'success' }), [saved]);
  assert.deepEqual(await reader.show(saved.id), saved);
});

test('CLI parses all filters, renders nested fields without object coercion, and combines them', async (t) => {
  const { root } = await fixture(t);
  const out = await runHistory(['list', '--store', root, '--repo', REPO_A, '--issue', '10', '--seat', 'coder',
    '--model', 'served-a', '--outcome', 'success', '--since', '2024-01-01', '--until', '2024-01-01']);
  assert.match(out, /session=s1.*run=r1.*repo=sha256-.*issue=10.*seat=coder.*model=served-a.*outcome=success/);
  assert.doesNotMatch(out, /run=r2|run=r3|\[object Object\]/);
  assert.match(formatList([]), /No history records matched/);
});

test('invalid CLI arguments fail before any store or git access', async () => {
  const options = { run: () => { throw new Error('unexpected git access'); } };
  for (const args of [['list', '--bogus', 'x'], ['list', '--issue'], ['list', '--store'],
    ['list', '--repo', 'a', '--repo', 'b'], ['show'], ['show', 'r1', 'r2'],
    ['show', 'r1', '--issue', '1'], ['show', '--store', 'dir'], ['unknown']]) {
    await assert.rejects(runHistory(args, options), /Use roster history/);
  }
  await assert.rejects(runHistory(['list', '--since', 'invalid'], options), /Invalid history since/);
  assert.deepEqual(parseListFlags(['--repo', REPO_A, '--store', 'dir']), { repository: REPO_A, storePath: 'dir' });
});

test('filesystem and git resolution failures are not disguised as missing history', async (t) => {
  const { dir } = await fixture(t, { populate: false });
  await assert.rejects(runHistory(['list'], { run: async () => { throw new Error('git metadata unavailable'); } }), /git metadata unavailable/);
  await assert.rejects(resolveHistoryRoot({ run: async () => ({ stdout: ' ' }) }), /Could not resolve/);
  const file = path.join(dir, 'not-a-directory');
  await writeFile(file, 'data');
  await assert.rejects(loadProvenanceRecords({ storePath: file }), /must be a directory/);
});

test('two distinct repositories with different origins never collide in history queries', async (t) => {
  const temp = await mkdtemp(path.join(tmpdir(), 'roster-hist-distinct-'));
  t.after(() => rm(temp, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { stdio: 'pipe' });
  // Same path suffix, distinct hosts: path-based identities would collide here,
  // origin-based identities must not.
  for (const [name, origin] of [
    ['a', 'https://example.invalid/same/widget-a.git'],
    ['b', 'https://other.invalid/same/widget-b.git'],
  ]) {
    const dir = path.join(temp, name);
    git('init', '-q', dir);
    git('-C', dir, 'remote', 'add', 'origin', origin);
    git('-C', dir, '-c', 'user.name=t', '-c', 'user.email=t@example.com',
      'commit', '-q', '--allow-empty', '-m', 'seed');
  }
  const identityA = await resolveRepoIdentity({ repoRoot: path.join(temp, 'a') });
  const identityB = await resolveRepoIdentity({ repoRoot: path.join(temp, 'b') });
  assert.notEqual(identityA, identityB);
  const records = [
    { id: 'rec-a', version: 1, runId: 'run-a', sessionId: 'sess-a', repoIdentity: identityA, event: 'completed', createdAt: JAN, payload: {} },
    { id: 'rec-b', version: 1, runId: 'run-b', sessionId: 'sess-b', repoIdentity: identityB, event: 'completed', createdAt: FEB, payload: {} },
  ];
  assert.deepEqual(ids(filterRepositories(records, [identityA])), ['run-a']);
  assert.deepEqual(ids(filterRepositories(records, [identityB])), ['run-b']);
  assert.deepEqual(ids(filterRecords(records, { repository: identityA })), ['run-a']);
  assert.deepEqual(ids(filterRecords(records, { repository: identityB })), ['run-b']);
  assert.deepEqual(ids(filterRepositories(records, [identityA, identityB])), ['run-a', 'run-b']);
  // A query for one repository never returns the other repository's record.
  assert.deepEqual(ids(filterRepositories(records, identityA)), ['run-a']);
  assert.equal(ids(filterRecords(records, { repository: identityB })).includes('run-a'), false);
});

test('typed history survives reclones and transport changes but excludes a distinct repository', async (t) => {
  const { dir, root } = await fixture(t, { populate: false });
  const repos = ['first', 'reclone', 'other'].map((name) => path.join(dir, name));
  const remotes = [
    'https://git.example.invalid/team/widget.git',
    'git@git.example.invalid:team/widget.git',
    'https://git.example.invalid/other/widget.git',
  ];
  for (let index = 0; index < repos.length; index++) {
    execFileSync('git', ['init', '-q', repos[index]], { stdio: 'pipe' });
    execFileSync('git', ['-C', repos[index], 'remote', 'add', 'origin', remotes[index]], { stdio: 'pipe' });
  }
  const stores = repos.map((repoRoot) => createProvenanceStore({ root, repoRoot }));
  await stores[0].recordEvent({
    runId: 'before-reclone', sessionId: 'original', event: 'completed', payload: { outcome: 'pass' },
  });
  await stores[2].recordEvent({
    runId: 'other-repo', sessionId: 'other', event: 'completed', payload: { outcome: 'pass' },
  });
  assert.deepEqual(ids(await stores[1].query()), ['before-reclone']);
  assert.deepEqual(ids(await stores[2].query()), ['other-repo']);
  const identity = await stores[1].identity();
  assert.deepEqual(ids(await createHistoryReader({ root }).list({ repository: identity })), ['before-reclone']);
});

test('deleting a checkout and cloning again preserves its durable typed history', async (t) => {
  const { dir, root } = await fixture(t, { populate: false });
  const origin = path.join(dir, 'origin');
  const first = path.join(dir, 'first');
  const reclone = path.join(dir, 'reclone');
  const git = (...args) => execFileSync('git', args, { stdio: 'pipe' });
  git('init', '-q', origin);
  git('-C', origin, '-c', 'user.name=t', '-c', 'user.email=t@example.com',
    'commit', '-q', '--allow-empty', '-m', 'seed');
  git('clone', '-q', '--no-local', origin, first);
  const before = createProvenanceStore({ root, repoRoot: first });
  const identity = await before.identity();
  await before.recordEvent({
    runId: 'preserved-after-delete', sessionId: 'first-clone', event: 'failure',
    payload: { outcome: 'failure' },
  });
  const bytes = await snapshot(root);
  await rm(first, { recursive: true, force: true });
  git('clone', '-q', '--no-local', origin, reclone);
  const after = createProvenanceStore({ root, repoRoot: reclone });
  assert.equal(await after.identity(), identity);
  const records = await after.query();
  assert.deepEqual(ids(records), ['preserved-after-delete']);
  assert.equal(records[0].event, 'failure');
  assert.equal(records[0].payload.outcome, 'failure');
  assert.deepEqual(await snapshot(root), bytes);
});

test('actual CLI list and show use --store with no network or real remotes', async (t) => {
  const { root } = await fixture(t);
  const cli = path.resolve('src', 'cli.mjs');
  const list = await execute(process.execPath, [cli, 'history', 'list', '--store', root, '--repo', REPO_B]);
  assert.match(list.stdout, /session=s2.*run=r2/);
  assert.doesNotMatch(list.stdout, /run=r1/);
  const show = await execute(process.execPath, [cli, 'history', 'show', 's1', '--store', root]);
  assert.match(show.stdout, /session=s1.*run=r1/);
});
