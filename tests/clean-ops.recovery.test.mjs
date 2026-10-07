import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

import { TRASH_PREFIX, pruneProvenance, runClean } from '../src/lib/clean-ops.mjs';
import { SEGMENT_NAME, openProvenanceStore } from '../src/lib/provenance-store.mjs';

const SECRET = 'test-only-private-api-key';

function stateRoot(t) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'clean-recovery-'));
  t.after(() => fsp.rm(tmp, { recursive: true, force: true }).catch(() => {}));
  const root = path.join(tmp, 'state');
  fs.mkdirSync(root);
  return { tmp, root, handle: { scope: 'worktree', root } };
}

function write(root, relative, content = SECRET) {
  const file = path.join(root, relative);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content);
}

function linkDir(target, link) {
  try {
    fs.symlinkSync(target, link, process.platform === 'win32' ? 'junction' : 'dir');
    return true;
  } catch {
    return false;
  }
}

test('repeated clean is idempotent and leaves no trash behind', async (t) => {
  const { root, handle } = stateRoot(t);
  write(root, 'runs/r1/run.log');
  write(root, 'memory.jsonl');
  const first = await runClean(handle, { execute: true, yes: true });
  assert.deepEqual(first.removed.map(({ target }) => path.basename(target)), ['memory.jsonl', 'runs']);
  assert.deepEqual(first.failed, []);
  assert.deepEqual(fs.readdirSync(root), []);
  const again = await runClean(handle, { execute: true, yes: true });
  assert.deepEqual([again.removed, again.failed, again.recordCount], [[], [], 0]);
});

test('trash left by an interrupted clean is previewed and swept by the next clean', async (t) => {
  const { root, handle } = stateRoot(t);
  write(root, `${TRASH_PREFIX}1-abc-runs/r1/run.log`);
  write(root, 'notes.json');
  const preview = await runClean(handle);
  assert.deepEqual(preview.included.map(({ name }) => name), [`${TRASH_PREFIX}1-abc-runs`, 'notes.json']);
  const done = await runClean(handle, { execute: true, yes: true });
  assert.equal(done.removed.length, 2);
  assert.deepEqual(fs.readdirSync(root), []);
});

test('a failing entry does not strand the rest and is reported by name and code', async (t) => {
  const { tmp, root, handle } = stateRoot(t);
  const outside = path.join(tmp, 'outside');
  write(outside, 'keep.txt');
  write(root, 'a/one');
  write(root, 'b/two');
  if (!linkDir(outside, path.join(root, 'link'))) {
    t.skip('directory links are unavailable');
    return;
  }
  const events = [];
  await assert.rejects(runClean(handle, { execute: true, yes: true, onEvent: (event) => events.push(event) }),
    (error) => {
      assert.equal(error.code, 'CLEAN_OPS_PARTIAL_CLEAN');
      assert.deepEqual(error.details.report.removed.map(({ target }) => path.basename(target)), ['a', 'b']);
      assert.deepEqual(error.details.report.failed, [{ name: 'link', code: 'CLEAN_OPS_PATH_ESCAPE' }]);
      assert.match(error.message, /removed 2 and failed 1 of 3[\s\S]*link: CLEAN_OPS_PATH_ESCAPE/);
      return true;
    });
  assert.ok(fs.existsSync(path.join(outside, 'keep.txt')));
  assert.deepEqual(events.map(({ entry, outcome }) => `${entry}:${outcome}`), ['a:removed', 'b:removed', 'link:failed']);
  // A retry is idempotent for what already went and still refuses the link.
  await assert.rejects(runClean(handle, { execute: true, yes: true }),
    (error) => error.details.report.removed.length === 0 && error.details.report.failed.length === 1);
  fs.rmSync(path.join(root, 'link'), { recursive: false, force: true });
});

test('clean events carry identifiers and outcomes only, and a failed logger is reported', async (t) => {
  const { root, handle } = stateRoot(t);
  write(root, 'secret.json');
  const events = [];
  await assert.rejects(runClean(handle, { execute: true, yes: true, onEvent: async (event) => {
    events.push(event);
    throw new Error('logger down');
  } }), (error) => error.code === 'CLEAN_OPS_LOG_FAILED' && error.details.cause.message === 'logger down');
  assert.deepEqual(fs.readdirSync(root), []);
  assert.deepEqual(events, [{ type: 'clean', scope: 'worktree', entry: 'secret.json', outcome: 'removed' }]);
  const serialized = JSON.stringify(events);
  assert.doesNotMatch(serialized, new RegExp(SECRET));
  assert.equal(serialized.includes(root), false);
});

test('cleanup redacts configured secrets in entry identifiers before emitting events', async (t) => {
  const { root, handle } = stateRoot(t);
  write(root, `${SECRET}.json`);
  const events = [];
  await runClean(handle, { execute: true, yes: true, env: { ROSTER_API_KEY: SECRET },
    onEvent: (event) => events.push(event) });
  assert.equal(JSON.stringify(events).includes(SECRET), false);
  assert.equal(events[0].outcome, 'removed');
});

test('index synchronization failure reports persistent deletions and is recoverable', async (t) => {
  const { tmp } = stateRoot(t);
  const store = path.join(tmp, 'provenance');
  const opened = openProvenanceStore(store);
  await opened.appendRecord({ id: 'gone', version: 1, section: 'curated-memory', memory: SECRET });
  const segment = path.join(store, SEGMENT_NAME);
  const stale = fs.readFileSync(segment, 'utf8');
  fs.rmSync(segment);
  fs.mkdirSync(segment);
  await assert.rejects(pruneProvenance(store, { target: 'curated-memory', execute: true, yes: true }),
    (error) => error.code === 'CLEAN_OPS_PARTIAL_CLEAN' &&
      error.details.report.removed[0] === 'gone' && Boolean(error.details.report.indexFailure));
  assert.equal(fs.existsSync(path.join(store, 'log', 'gone.json')), false);
  fs.rmdirSync(segment);
  fs.writeFileSync(segment, stale);
  await pruneProvenance(store, { target: 'curated-memory', execute: true, yes: true });
  assert.equal(fs.readFileSync(segment, 'utf8'), '');
});

test('provenance prune finishes an index left stale by an interrupted prune', async (t) => {
  const { tmp } = stateRoot(t);
  const store = path.join(tmp, 'provenance');
  const opened = openProvenanceStore(store);
  await opened.appendRecord({ id: 'kept', version: 1, section: 'raw-history' });
  await opened.appendRecord({ id: 'gone', version: 1, section: 'curated-memory', memory: SECRET });
  // Interrupted after the record unlink and before the index rebuild.
  fs.rmSync(path.join(store, 'log', 'gone.json'));
  assert.match(fs.readFileSync(path.join(store, SEGMENT_NAME), 'utf8'), new RegExp(SECRET));
  const events = [];
  const done = await pruneProvenance(store, { target: 'curated-memory', execute: true, yes: true,
    onEvent: (event) => events.push(event) });
  assert.deepEqual([done.removed, done.failed], [[], []]);
  const segment = fs.readFileSync(path.join(store, SEGMENT_NAME), 'utf8');
  assert.doesNotMatch(segment, new RegExp(SECRET));
  assert.match(segment, /"kept"/);
  assert.deepEqual(events, []);
  // An already-consistent index is left exactly as it is.
  const before = fs.statSync(path.join(store, SEGMENT_NAME)).mtimeMs;
  await pruneProvenance(store, { target: 'curated-memory', execute: true, yes: true });
  assert.equal(fs.statSync(path.join(store, SEGMENT_NAME)).mtimeMs, before);
});

test('provenance prune events name record ids, never memory content', async (t) => {
  const { tmp } = stateRoot(t);
  const store = path.join(tmp, 'provenance');
  await openProvenanceStore(store).appendRecord({ id: 'memory-1', version: 1, section: 'curated-memory', memory: SECRET });
  const events = [];
  await pruneProvenance(store, { target: 'curated-memory', execute: true, yes: true, onEvent: (event) => events.push(event) });
  assert.deepEqual(events, [{ type: 'clean', scope: 'machine', entry: 'memory-1', outcome: 'removed' }]);
});
