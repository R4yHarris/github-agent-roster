import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { archivedRunScope, latestArchivedReview } from '../src/lib/run-artifacts.mjs';
import { restoreRunScope } from '../src/lib/builtin.mjs';

test('the newest archived REVIEW.md is found even when a later interrupted run archived none', async (t) => {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-archive-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const common = path.join(root, '.git');
  const archive = (name, files) => {
    const directory = path.join(common, 'roster-artifacts', 'issue-7', name);
    mkdirSync(directory, { recursive: true });
    for (const [file, text] of Object.entries(files)) writeFileSync(path.join(directory, file), text);
  };
  archive('1000-aaaaaaaaaaaa', { 'REVIEW.md': 'old' });
  archive('2000-bbbbbbbbbbbb', { 'REVIEW.md': 'newest review' });
  archive('3000-cccccccccccc', { 'RESULT.md': 'interrupted' });
  const git = async () => common;
  assert.equal(await latestArchivedReview(root, { task: 'issue-7', git }), 'newest review');
  assert.equal(await latestArchivedReview(root, { task: 'issue-7', git, accept: (text) => text !== 'newest review' }), 'old');
  assert.equal(await latestArchivedReview(root, { task: 'issue-8', git }), null);
  assert.equal(await latestArchivedReview(root, { task: '../escape', git }), null);
});

test('a resumed run restores recorded repair and expansion scope only for files still changed', async (t) => {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-archive-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const common = path.join(root, '.git');
  const archive = (name, text) => {
    const directory = path.join(common, 'roster-artifacts', 'issue-7', name);
    mkdirSync(directory, { recursive: true });
    writeFileSync(path.join(directory, 'RESULT.md'), text);
  };
  archive('1000-aaaaaaaaaaaa', 'Additional failing-test scope: tests/onboard.test.mjs, tests/gone.test.mjs\n' +
    'Files outside planned scope: probe.mjs, src/helper.mjs\n');
  archive('2000-bbbbbbbbbbbb', 'Additional failing-test scope: (none)\r\nFiles outside planned scope: .env\r\n');
  const outputs = {
    'rev-parse': common,
    diff: 'tests/onboard.test.mjs\0src/paths.mjs\0.env\0',
    'ls-files': 'src/helper.mjs\0',
  };
  const git = async (args) => outputs[args[0]];
  assert.deepEqual(await archivedRunScope(root, { task: 'issue-7', git }), {
    repairFiles: ['tests/gone.test.mjs', 'tests/onboard.test.mjs'],
    scopeFiles: ['.env', 'probe.mjs', 'src/helper.mjs'],
  });
  assert.deepEqual(await restoreRunScope(root, 'issue-7', git, ['src/paths.mjs']), {
    repairFiles: ['tests/onboard.test.mjs'],
    scopeFiles: ['src/helper.mjs'],
  });
  assert.deepEqual(await archivedRunScope(root, { task: '../escape', git }), { repairFiles: [], scopeFiles: [] });
});
