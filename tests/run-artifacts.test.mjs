import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { latestArchivedReview } from '../src/lib/run-artifacts.mjs';

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
