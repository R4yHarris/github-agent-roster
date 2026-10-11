import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { execFileSync } from 'node:child_process';
import { execute } from '../src/runtime/exec.mjs';
import { createTools } from '../src/runtime/tools.mjs';

function fixture(t, count = 1) {
  const root = mkdtempSync(path.join(process.cwd(), '.baseline-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(path.join(root, 'tests'));
  const files = Array.from({ length: count }, (_, i) => 'tests/outside' + i + '.test.mjs');
  for (const file of files) writeFileSync(path.join(root, file), "import test from 'node:test';\ntest('failure', () => { throw new Error('fixture failure'); });\n");
  return { root, files };
}
const failure = (files) => Object.assign(new Error('tests failed'), { code: 1,
  stdout: files.map((file, i) => 'not ok ' + (i + 1) + ' - failure\n  location: ' + file + ':1:1').join('\n'), stderr: '' });

for (const full of [true, false]) {
  test('real failing tests retain unavailable baseline, full=' + full, async (t) => {
    const { root, files } = fixture(t);
    const events = [];
    const tools = await createTools({ worktree: root, allowedFiles: ['src/app.mjs'], initialRepairFiles: files,
      onEvent: (event) => events.push(event), runCommand: async (program, args, options) => {
        if (program === 'git') throw Object.assign(new Error('fixture has no Git base'), { code: 128 });
        return execute(program, args, options);
      } });
    const result = await tools.run_test({}, { full });
    assert.equal(result.exit_code, 1);
    assert.deepEqual(result.preexisting_files, []);
    assert.deepEqual(result.unclassified_files, files);
    assert.match(result.baseline_diagnostic, /unavailable/i);
    assert.ok(events.some((event) => event.type === 'baseline-unavailable'));
  });
}

test('comparison cap retains unchecked failures without baseline claims', async (t) => {
  const { root, files } = fixture(t, 9);
  const tools = await createTools({ worktree: root, allowedFiles: ['src/app.mjs'],
    runCommand: async (program) => {
      if (program === 'git') throw new Error('base unavailable');
      throw failure(files);
    } });
  const result = await tools.run_test({}, { full: true });
  assert.deepEqual(result.preexisting_files, []);
  assert.deepEqual(result.unclassified_files, files);
  assert.match(result.baseline_diagnostic, /cap|limit/i);
});

for (const interruption of [{ code: 'ENOENT' }, { code: 1, killed: true }, { code: 1, signal: 'SIGTERM' }, { code: 1, stderr: 'node: bad option' }]) {
  test('interrupted or infrastructure baseline is unknown: ' + JSON.stringify(interruption), async (t) => {
    const { root, files } = fixture(t);
    let nodeRuns = 0;
    const tools = await createTools({ worktree: root, allowedFiles: ['src/app.mjs'],
      runCommand: async (program, args, options) => {
        if (program === 'git') {
          if (args[0] === 'worktree' && args[1] === 'add') mkdirSync(args[3], { recursive: true });
          return { stdout: '', stderr: '' };
        }
        nodeRuns++;
        if (options.cwd === root) throw failure(files);
        throw Object.assign(new Error('baseline unavailable'), interruption);
      } });
    const result = await tools.run_test({}, { full: true });
    assert.equal(nodeRuns, 3);
    assert.deepEqual(result.preexisting_files, []);
    assert.deepEqual(result.unclassified_files, files);
    assert.match(result.baseline_diagnostic, /unavailable|unclassified/i);
  });
}

test('scoped retry measures changed-source regression against actual Git base', async (t) => {
  const { root, files } = fixture(t);
  mkdirSync(path.join(root, 'src'));
  writeFileSync(path.join(root, 'src', 'app.mjs'), 'export const value = 1;\n');
  writeFileSync(path.join(root, files[0]), "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { value } from '../src/app.mjs';\ntest('consumer', () => assert.equal(value, 1));\n");
  const git = (...args) => execFileSync('git', ['-c', 'user.name=fixture', '-c', 'user.email=fixture@example.invalid',
    '-c', 'commit.gpgsign=false', ...args], { cwd: root, stdio: 'pipe' });
  git('init', '-q'); git('add', 'src', 'tests'); git('commit', '-qm', 'baseline fixture');
  writeFileSync(path.join(root, 'src', 'app.mjs'), 'export const value = 2;\n');
  const tools = await createTools({ worktree: root, allowedFiles: ['src/app.mjs'], initialRepairFiles: files });
  const result = await tools.run_test();
  assert.equal(result.exit_code, 1);
  assert.deepEqual(result.regression_files, files);
  assert.deepEqual(result.preexisting_files, []);
  assert.deepEqual(result.unclassified_files, []);
  assert.equal(git('worktree', 'list').toString().trim().split('\n').length, 1);
});
