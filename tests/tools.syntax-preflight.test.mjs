import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import test from 'node:test';
import { createTools } from '../src/runtime/tools.mjs';

const execute = promisify(execFile);

function fixture(t) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-syntax-preflight-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  mkdirSync(path.join(worktree, 'src'));
  mkdirSync(path.join(worktree, 'tests'));
  return worktree;
}

test('final verification reports malformed scoped JavaScript before launching the suite', async (t) => {
  const worktree = fixture(t);
  writeFileSync(path.join(worktree, 'src', 'app.mjs'), 'export const value = `broken ${;\n');
  const calls = [];
  const tools = await createTools({ worktree, allowedFiles: ['src/app.mjs'], runCommand: async (...args) => {
    calls.push(args[1]);
    return execute(...args);
  } });
  const failed = await tools.run_test({}, { full: true });
  assert.equal(failed.exit_code, 1);
  assert.equal(failed.syntax_check, true);
  assert.deepEqual(failed.failing_files, ['src/app.mjs']);
  assert.match(failed.stderr, /SyntaxError/);
  assert.deepEqual(calls, [['--check', 'src/app.mjs']]);
  await tools.read_file({ path: 'src/app.mjs' });
  await tools.write_file({ path: 'src/app.mjs', content: 'export const value = 1;\n' });
  const passed = await tools.run_test({}, { full: true });
  assert.equal(passed.exit_code, 0);
  assert.equal(calls.at(-1)[0], '--test', 'valid syntax does not replace the suite');
});

test('wildcard-scope writes are checked and deleted planned files are skipped', async (t) => {
  const worktree = fixture(t);
  const calls = [];
  const tools = await createTools({ worktree, allowedFiles: ['src/**', 'src/deleted.mjs'],
    runCommand: async (...args) => { calls.push(args[1]); return execute(...args); } });
  await tools.list_dir({ path: 'src' });
  await tools.write_file({ path: 'src/new.mjs', content: 'export const value = ;\n' });
  const result = await tools.run_test({}, { full: true });
  assert.equal(result.exit_code, 1);
  assert.deepEqual(result.failing_files, ['src/new.mjs']);
  assert.deepEqual(calls, [['--check', 'src/new.mjs']]);
});

test('syntax preflight strips publishing credentials and propagates infrastructure errors', async (t) => {
  const worktree = fixture(t);
  writeFileSync(path.join(worktree, 'src', 'app.mjs'), 'export const value = 1;\n');
  const failure = new Error('process could not start');
  failure.code = 'ENOENT';
  const tools = await createTools({ worktree, allowedFiles: ['src/app.mjs'],
    env: { ...process.env, GITHUB_APP_ID: 'sentinel', GITHUB_APP_PRIVATE_KEY_PATH: 'sentinel',
      ROSTER_API_KEY: 'sentinel', GH_TOKEN: 'sentinel', GITHUB_TOKEN: 'sentinel' },
    runCommand: async (_program, args, options) => {
      assert.equal(args[0], '--check');
      for (const name of ['GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH', 'ROSTER_API_KEY', 'GH_TOKEN', 'GITHUB_TOKEN']) {
        assert.equal(options.env[name], undefined);
      }
      throw failure;
    } });
  await assert.rejects(tools.run_test({}, { full: true }), (error) => error.cause === failure);
});

test('syntax preflight cancellation never launches the full suite', async (t) => {
  const worktree = fixture(t);
  writeFileSync(path.join(worktree, 'src', 'app.mjs'), 'export const value = 1;\n');
  const controller = new AbortController();
  const calls = [];
  const tools = await createTools({ worktree, allowedFiles: ['src/app.mjs'], signal: controller.signal,
    runCommand: async (_program, args, options) => {
      calls.push(args);
      assert.equal(options.signal, controller.signal);
      controller.abort();
      throw Object.assign(new Error('aborted'), { code: 'ABORT_ERR' });
    } });
  await assert.rejects(tools.run_test({}, { full: true }), { code: 'ROSTER_CANCELLED' });
  assert.deepEqual(calls, [['--check', 'src/app.mjs']]);
});

test('syntax preflight timeout names its file and never launches the full suite', async (t) => {
  const worktree = fixture(t);
  writeFileSync(path.join(worktree, 'src', 'app.mjs'), 'export const value = 1;\n');
  const calls = [];
  const failure = Object.assign(new Error('timed out'), { code: 'ETIMEDOUT', killed: true });
  const tools = await createTools({ worktree, allowedFiles: ['src/app.mjs'],
    runCommand: async (_program, args, options) => {
      calls.push(args);
      assert.equal(options.timeout, 30_000);
      throw failure;
    } });
  await assert.rejects(tools.run_test({}, { full: true }), (error) => {
    assert.match(error.message, /node --check src\/app\.mjs timed out after 30 seconds/);
    assert.equal(error.cause.cause, failure);
    return true;
  });
  assert.deepEqual(calls, [['--check', 'src/app.mjs']]);
});
