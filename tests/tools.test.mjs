import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createTools, isAllowedFile, isForbiddenWrite, toolDefinitions } from '../src/runtime/tools.mjs';

function fixture(context) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-tools-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true }));
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  writeFileSync(path.join(worktree, '.env'), 'DO_NOT_READ=secret\n');
  mkdirSync(path.join(worktree, 'src'));
  return worktree;
}

test('limits reading, writing, and listing to worktree files allowed by TASK.md', async (context) => {
  const worktree = fixture(context);
  const tools = await createTools({ worktree, allowedFiles: ['README.md', 'src/**'] });
  assert.equal(await tools.read_file({ path: 'README.md' }), '# Example\n');
  assert.deepEqual((await tools.list_dir({ path: '.' })).map(({ name }) => name),
    ['README.md', 'src']);
  assert.deepEqual(await tools.write_file({ path: 'src/new.mjs', content: 'export const ok = true;\n' }),
    { path: 'src/new.mjs', bytes: 24 });
  assert.equal(readFileSync(path.join(worktree, 'src', 'new.mjs'), 'utf8'), 'export const ok = true;\n');
  await assert.rejects(tools.write_file({ path: 'docs/no.md', content: '' }), /not allowed/);
  for (const file of ['../outside.md', path.join(worktree, '..', 'outside.md'), '.env',
    'nested/.env.local', 'key.pem', 'src/agent-policy.yml', '.github/workflows/build.yml',
    '.git/config', 'TASK.md', 'ASSIGNMENT.md', 'RESULT.md', 'RECIPE.yml', 'ESTIMATE.md', '.roster/evals.jsonl']) {
    await assert.rejects(tools.write_file({ path: file, content: 'bad' }), /relative|inside|secret|not allowed/i, file);
  }
  await assert.rejects(tools.read_file({ path: '.env' }), /secrets/);
  await assert.rejects(tools.read_file({ path: '../outside.md' }), /inside/);
  await assert.rejects(tools.write_file({ path: 'README.md', content: 42 }), /must be text/);
  await assert.rejects(tools.read_file({ path: 'README.md', ignored: true }), /Tool arguments/);
  assert.equal(isForbiddenWrite('other/.github/workflows/ci.yml'), true);
  assert.equal(isAllowedFile('src/other.mjs', ['src/**']), true);
  assert.equal(isAllowedFile('docs/file.md', ['src/**']), false);
});

test('coder tools cannot write human evaluations even with broad task scope or a test child process', async (context) => {
  const worktree = fixture(context);
  const tools = await createTools({ worktree, allowedFiles: ['**/*'] });
  assert.equal(Object.hasOwn(tools, 'eval'), false);
  assert.equal(toolDefinitions.some(({ function: tool }) => /eval/i.test(tool.name)), false);
  await assert.rejects(tools.write_file({ path: '.roster/evals.jsonl', content: '{"verdict":"accept"}\n' }),
    /not allowed/);
  const evaluator = new URL('../src/lib/eval.mjs', import.meta.url).href;
  writeFileSync(path.join(worktree, 'no-self-eval.test.mjs'),
    "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    `import { recordEvaluation } from ${JSON.stringify(evaluator)};\n` +
    "test('no self evaluation', async () => {\n" +
    "  await assert.rejects(recordEvaluation('roster-42-coder', 'accept', '3', 'y'), /human-only/);\n" +
    "});\n");
  const result = await tools.run_test();
  assert.equal(result.exit_code, 0, result.stderr || result.stdout);
});

test('list_dir hides protected entries and refuses their paths while writes stay denied', async (context) => {
  const worktree = fixture(context);
  mkdirSync(path.join(worktree, '.github', 'workflows'), { recursive: true });
  mkdirSync(path.join(worktree, 'vendor', 'github-agent-contracts'), { recursive: true });
  mkdirSync(path.join(worktree, 'src', '.env.private'));
  for (const file of ['agent-policy.yml', '.github/workflows/ci.yml',
    'vendor/github-agent-contracts/checker.mjs', 'src/key.pem', 'src/agent-policy.yml']) {
    writeFileSync(path.join(worktree, file), 'protected');
  }
  const tools = await createTools({ worktree, allowedFiles: ['**/*'] });
  assert.deepEqual((await tools.list_dir({ path: '.' })).map(({ name }) => name),
    ['.github', 'README.md', 'src', 'vendor']);
  assert.deepEqual((await tools.list_dir({ path: '.github' })).map(({ name }) => name), []);
  assert.deepEqual((await tools.list_dir({ path: 'vendor' })).map(({ name }) => name), []);
  assert.deepEqual((await tools.list_dir({ path: 'src' })).map(({ name }) => name), []);
  for (const file of ['..', path.dirname(worktree), '.env', 'agent-policy.yml',
    '.github/workflows', 'src/key.pem', 'src/.env.private', 'src/agent-policy.yml',
    'vendor/github-agent-contracts']) {
    await assert.rejects(tools.list_dir({ path: file }),
      /relative|inside|secrets|Listing protected/i, file);
  }
  for (const file of ['agent-policy.yml', '.github/workflows/ci.yml',
    'vendor/github-agent-contracts/checker.mjs', 'src/key.pem', 'src/agent-policy.yml']) {
    await assert.rejects(tools.write_file({ path: file, content: 'changed' }), /not allowed|secrets/);
    assert.equal(readFileSync(path.join(worktree, file), 'utf8'), 'protected');
  }
  assert.equal(isForbiddenWrite('vendor/github-agent-contracts/scripts/agent-pr.mjs'), true);
});

test('refuses symlink paths rather than following them out of the worktree', async (context) => {
  const worktree = fixture(context);
  const outsideDirectory = mkdtempSync(path.join(tmpdir(), 'roster-tools-outside-'));
  context.after(() => rmSync(outsideDirectory, { recursive: true, force: true }));
  const outside = path.join(outsideDirectory, 'outside-file.txt');
  const link = path.join(worktree, 'src', 'link.mjs');
  writeFileSync(outside, 'outside');
  try {
    symlinkSync(outside, link);
  } catch (error) {
    if (['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) {
      context.skip('Creating symlinks is unavailable on this system.');
      return;
    }
    throw error;
  }
  const tools = await createTools({ worktree, allowedFiles: ['src/**'] });
  await assert.rejects(tools.read_file({ path: 'src/link.mjs' }), /symlinks/);
  await assert.rejects(tools.write_file({ path: 'src/link.mjs', content: 'bad' }), /symlinks/);
  assert.equal(readFileSync(outside, 'utf8'), 'outside');
});

test('run_test uses node --test with a 60s timeout and strips API and GitHub credentials', async (context) => {
  const worktree = fixture(context);
  let options;
  const tools = await createTools({
    worktree, allowedFiles: ['README.md'], apiKeyEnv: 'CUSTOM_KEY',
    env: { PATH: process.env.PATH, CUSTOM_KEY: 'secret', GH_TOKEN: 'token',
      NODE_TEST_CONTEXT: 'child-v8',
      GITHUB_APP_ID: '1', GITHUB_APP_PRIVATE_KEY_PATH: 'key.pem' },
    runCommand: async (program, args, received) => {
      assert.equal(program, process.execPath);
      assert.deepEqual(args, ['--test']);
      options = received;
      return { stdout: 'tests pass', stderr: '' };
    },
  });
  assert.deepEqual(await tools.run_test({}), { exit_code: 0, stdout: 'tests pass', stderr: '' });
  assert.equal(options.cwd, worktree);
  assert.equal(options.timeout, 60_000);
  assert.equal(options.env.PATH, process.env.PATH);
  assert.equal(options.env.ROSTER_SEAT, 'coder');
  for (const key of ['CUSTOM_KEY', 'GH_TOKEN', 'GITHUB_APP_ID',
    'GITHUB_APP_PRIVATE_KEY_PATH', 'NODE_TEST_CONTEXT']) {
    assert.equal(options.env[key], undefined);
  }
  await assert.rejects(tools.run_test({ command: 'echo secret' }), /Tool arguments/);

  const failing = await createTools({ worktree, allowedFiles: ['README.md'],
    runCommand: async () => { throw Object.assign(new Error('tests failed'), {
      code: 1, stdout: 'not ok', stderr: 'assertion failed',
    }); } });
  assert.deepEqual(await failing.run_test(), {
    exit_code: 1, stdout: 'not ok', stderr: 'assertion failed',
  });
  const timedOut = await createTools({ worktree, allowedFiles: ['README.md'],
    runCommand: async () => { throw Object.assign(new Error('timeout'), { code: 'ETIMEDOUT' }); } });
  await assert.rejects(timedOut.run_test(), /timed out after 60 seconds/);
});

test('run_test actually executes Node tests from the worktree', async (context) => {
  const worktree = fixture(context);
  writeFileSync(path.join(worktree, 'example.test.mjs'),
    "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    "import { writeFileSync } from 'node:fs';\n" +
    "test('example', () => { assert.equal(2 + 2, 4); writeFileSync('ran-marker.txt', 'yes'); });\n");
  const tools = await createTools({ worktree, allowedFiles: ['README.md'] });
  const result = await tools.run_test();
  assert.equal(result.exit_code, 0, result.stderr);
  assert.equal(readFileSync(path.join(worktree, 'ran-marker.txt'), 'utf8'), 'yes');
});
