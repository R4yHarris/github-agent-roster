import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { captureCheckpoint, listCheckpoints, rewindCheckpoint, branchHasPublishedPr } from '../src/lib/checkpoints.mjs';
import { createTools } from '../src/runtime/tools.mjs';
import { formatHelp } from '../src/shell/commands.mjs';

function fixture(t) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-checkpoint-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  execFileSync('git', ['init', '--quiet', '-b', 'issue-108'], { cwd: worktree });
  writeFileSync(path.join(worktree, 'README.md'), '# Original\n');
  writeFileSync(path.join(worktree, 'TASK.md'), '# Task remains\n');
  writeFileSync(path.join(worktree, 'PLAN.md'), '# Plan remains\n');
  writeFileSync(path.join(worktree, 'run.log'), 'Log remains\n');
  return { worktree, task: 'issue-108', allowedFiles: ['README.md'], env: {} };
}

test('stash-free tree checkpoint restores README but preserves task, plan and logs', async (t) => {
  const options = fixture(t);
  const checkpoint = await captureCheckpoint(options);
  assert.equal(checkpoint.number, 1);
  assert.ok(existsSync(path.join(options.worktree, '.roster', 'checkpoints', '108', '1')));
  assert.deepEqual(await listCheckpoints(options), [checkpoint]);
  writeFileSync(path.join(options.worktree, 'README.md'), '# Edited\n');
  await rewindCheckpoint({ ...options, number: 1, hasPublishedPr: async () => false });
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Original\n');
  assert.equal(readFileSync(path.join(options.worktree, 'TASK.md'), 'utf8'), '# Task remains\n');
  assert.equal(readFileSync(path.join(options.worktree, 'PLAN.md'), 'utf8'), '# Plan remains\n');
  assert.equal(readFileSync(path.join(options.worktree, 'run.log'), 'utf8'), 'Log remains\n');
  assert.equal(execFileSync('git', ['stash', 'list'], { cwd: options.worktree, encoding: 'utf8' }), '');
  assert.match(formatHelp('rewind'), /Aliases: \/undo/);
});

test('pre-write hook checkpoints actual coder writes only after scope validation', async (t) => {
  const options = fixture(t);
  const tools = await createTools({ ...options,
    beforeWrite: ({ allowedFiles }) => captureCheckpoint({ ...options, allowedFiles }) });
  await assert.rejects(tools.write_file({ path: 'TASK.md', content: 'denied' }), /not allowed/);
  assert.equal((await listCheckpoints(options)).length, 0);
  await tools.write_file({ path: 'README.md', content: '# Changed\n' });
  await tools.write_file({ path: 'README.md', content: '# Changed twice\n' });
  assert.equal((await listCheckpoints(options)).length, 2);
  await rewindCheckpoint({ ...options, hasPublishedPr: async () => false });
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Changed\n');
});

test('a published branch or failed PR verification refuses all product restoration', async (t) => {
  const options = fixture(t);
  await captureCheckpoint(options);
  writeFileSync(path.join(options.worktree, 'README.md'), '# Published change\n');
  await assert.rejects(rewindCheckpoint({ ...options, hasPublishedPr: async () => true }), /published PR/);
  await assert.rejects(rewindCheckpoint({ ...options, published: true,
    hasPublishedPr: () => assert.fail('Known published status must refuse before GitHub') }), /published PR/);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Published change\n');
  const found = await branchHasPublishedPr({ worktree: options.worktree, env: {}, runCommand: async (program, args) =>
    program === 'gh' ? JSON.stringify([{ number: 3 }]) : args[0] === 'branch' ? 'issue-108\n'
      : 'https://github.com/example/project.git\n' });
  assert.equal(found, true);
});

test('rewind removes only checkpoint-covered newly created products and never private artifacts', async (t) => {
  const options = fixture(t);
  options.allowedFiles = ['README.md', 'src/**'];
  await captureCheckpoint(options);
  mkdirSync(path.join(options.worktree, 'src'));
  writeFileSync(path.join(options.worktree, 'src', 'created.mjs'), 'export {};\n');
  const tools = await createTools({ worktree: options.worktree, allowedFiles: ['**/*'] });
  await assert.rejects(tools.read_file({ path: '.roster/checkpoints/108/1' }), /refused/);
  await rewindCheckpoint({ ...options, hasPublishedPr: async () => false });
  assert.equal(existsSync(path.join(options.worktree, 'src', 'created.mjs')), false);
  assert.equal(existsSync(path.join(options.worktree, 'TASK.md')), true);
});
