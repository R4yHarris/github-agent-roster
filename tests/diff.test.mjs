import assert from 'node:assert/strict';
import test from 'node:test';
import { readDiffStatus } from '../src/lib/diff.mjs';

test('diff status excludes untracked files by default', async () => {
  let invocation;
  const output = await readDiffStatus({
    cwd: 'fixture',
    runCommand: async (...args) => {
      invocation = args;
      return { stdout: ' M README.md\n' };
    },
  });
  assert.equal(output, ' M README.md\n');
  assert.deepEqual(invocation, ['git', [
    'status', '--short', '--porcelain=v1', '--untracked-files=no',
  ], { cwd: 'fixture', encoding: 'utf8' }]);
});

test('diff status includes untracked files only when explicitly requested', async () => {
  let invocation;
  await readDiffStatus({
    cwd: 'fixture', untracked: true,
    runCommand: async (...args) => { invocation = args; return { stdout: '?? notes.txt\n' }; },
  });
  assert.deepEqual(invocation, ['git', [
    'status', '--short', '--porcelain=v1', '--untracked-files=all',
  ], { cwd: 'fixture', encoding: 'utf8' }]);
});
