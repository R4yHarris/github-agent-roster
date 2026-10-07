import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createTools, isAllowedFile, isForbiddenRead, isForbiddenWrite, taskAndRepairFiles, testFailureEvidence, toolDefinitions } from '../src/runtime/tools.mjs';

function fixture(context) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-tools-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true }));
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  writeFileSync(path.join(worktree, '.env'), 'DO_NOT_READ=secret\n');
  mkdirSync(path.join(worktree, 'src'));
  return worktree;
}

function docsCheck(worktree) {
  mkdirSync(path.join(worktree, 'tests'), { recursive: true });
  writeFileSync(path.join(worktree, 'tests', 'repl.test.mjs'), '');
}

test('read_file on a missing path names real similar siblings without leaking the host path', async (context) => {
  const worktree = fixture(context);
  mkdirSync(path.join(worktree, 'src', 'lib'), { recursive: true });
  for (const name of ['provenance-api.mjs', 'provenance-store.mjs', 'paths.mjs']) {
    writeFileSync(path.join(worktree, 'src', 'lib', name), '');
  }
  writeFileSync(path.join(worktree, '.envrc'), 'SECRET=1\n');
  const tools = await createTools({ worktree, allowedFiles: ['src/lib/history.mjs'] });
  await assert.rejects(tools.read_file({ path: 'src/lib/provenance.mjs' }), (error) => {
    assert.equal(error.message,
      'read_file: src/lib/provenance.mjs does not exist. Similar files: src/lib/provenance-api.mjs, src/lib/provenance-store.mjs');
    assert.ok(!error.message.includes(worktree));
    return true;
  });
  await assert.rejects(tools.read_file({ path: '.env.local' }), /Tool access to secrets/);
  await assert.rejects(tools.read_file({ path: 'missing/nowhere.mjs' }), /does not exist\. Use list_dir/);
});

test('every slice denies out-of-scope file reads, including fixtures and harness sources', async (context) => {
  const worktree = fixture(context);
  mkdirSync(path.join(worktree, 'docs'));
  mkdirSync(path.join(worktree, 'tests', 'fixtures'), { recursive: true });
  writeFileSync(path.join(worktree, 'docs', 'guide.md'), 'needle allowed\n');
  writeFileSync(path.join(worktree, 'tests', 'fixtures', 'planner.md'), 'needle fixture\n');
  writeFileSync(path.join(worktree, 'src', 'repl.mjs'), 'needle harness\n');
  writeFileSync(path.join(worktree, 'TASK.md'), 'Task scope\n');
  const tools = await createTools({ worktree, allowedFiles: ['docs/guide.md'], sliceReadsOnly: true });
  assert.equal(await tools.read_file({ path: 'docs/guide.md' }), 'needle allowed\n');
  assert.equal(await tools.read_file({ path: 'TASK.md' }), 'Task scope\n');
  for (const file of ['README.md', 'tests/fixtures/planner.md', 'src/repl.mjs']) {
    await assert.rejects(tools.read_file({ path: file }), /not allowed by TASK\.md slice scope/);
    await assert.rejects(tools.search_text({ path: file, query: 'needle' }), /not allowed/);
  }
  for (const directory of ['tests', 'tests/fixtures', 'src']) {
    await assert.rejects(tools.list_dir({ path: directory }), /not allowed/);
  }
  assert.deepEqual((await tools.list_dir({ path: '.' })).map(({ name }) => name), ['docs', 'TASK.md']);
  assert.deepEqual((await tools.search_text({ query: 'needle' })).matches,
    [{ path: 'docs/guide.md', line: 1, text: 'needle allowed' }]);
  const explicitlyAllowed = await createTools({ worktree,
    allowedFiles: ['tests/fixtures/planner.md', 'src/repl.mjs'], sliceReadsOnly: true });
  assert.match(await explicitlyAllowed.read_file({ path: 'tests/fixtures/planner.md' }), /fixture/);
  assert.match(await explicitlyAllowed.read_file({ path: 'src/repl.mjs' }), /harness/);
  const subtree = await createTools({ worktree, allowedFiles: ['docs/**'], sliceReadsOnly: true });
  assert.match(await subtree.read_file({ path: 'docs/guide.md' }), /allowed/);
  await assert.rejects(subtree.read_file({ path: 'src/repl.mjs' }), /not allowed/);
});

test('a real failed node test grants only its regular failing-test file for repair', async (context) => {
  const worktree = fixture(context);
  mkdirSync(path.join(worktree, 'tests'));
  const failing = 'tests/app.test.mjs';
  writeFileSync(path.join(worktree, failing),
    "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    "test('broken', () => assert.equal(1, 2));\n");
  writeFileSync(path.join(worktree, 'tests', 'unrelated.test.mjs'),
    "import test from 'node:test';\ntest('unrelated', () => {});\n");
  const tools = await createTools({ worktree, allowedFiles: ['src/app.mjs'], sliceReadsOnly: true });
  await assert.rejects(tools.read_file({ path: failing }), /not allowed/);
  const failed = await tools.run_test();
  assert.equal(failed.exit_code, 1);
  assert.deepEqual(failed.repair_files, [failing]);
  assert.deepEqual(failed.failing_files, [failing]);
  assert.match(await tools.read_file({ path: failing }), /assert.equal/);
  await tools.write_file({ path: failing,
    content: "import test from 'node:test';\ntest('broken', () => {});\n" });
  await assert.rejects(tools.write_file({ path: 'tests/unrelated.test.mjs', content: '' }), /not allowed/);
  assert.equal((await tools.run_test()).exit_code, 0);
});

test('test failure evidence keeps the failures and counts, not the passing head', () => {
  const passing = Array.from({ length: 300 }, (_, index) => `✔ passes ${index} (1ms)`).join('\n');
  const spec = `${passing}\nℹ tests 301\nℹ pass 300\nℹ fail 1\n\n✖ failing tests:\n\ntest at tests\\a.test.mjs:3:1\n` +
    '✖ broken (2ms)\n  AssertionError: marker\n';
  const evidence = testFailureEvidence(spec);
  assert.match(evidence, /ℹ fail 1/);
  assert.match(evidence, /failing tests:[\s\S]*AssertionError: marker/);
  assert.doesNotMatch(evidence, /passes 0 /);
  const tap = `ok 1 - fine\nnot ok 2 - broken\n  ---\n  error: 'marker'\n  ...\nok 3 - later\n`;
  assert.equal(testFailureEvidence(tap), "not ok 2 - broken\n  ---\n  error: 'marker'\n  ...");
  assert.equal(testFailureEvidence('x'.repeat(5000) + 'END', 100), `${'x'.repeat(97)}END`);
  assert.ok(testFailureEvidence(spec, 200).length <= 200);
});

test('regression classification mirrors initialized submodules into the base worktree', async (context) => {
  const worktree = fixture(context);
  const git = (...args) => execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.com', ...args],
    { cwd: worktree, encoding: 'utf8', stdio: 'pipe' });
  const write = (file, text) => {
    mkdirSync(path.dirname(path.join(worktree, ...file.split('/'))), { recursive: true });
    writeFileSync(path.join(worktree, ...file.split('/')), text);
  };
  const check = (expression) => "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    "import { value } from '../src/app.mjs';\nimport { ready } from '../deps/lib/index.mjs';\n" +
    `test('check', () => assert.ok(ready && ${expression}));\n`;
  write('.gitmodules', '[submodule "lib"]\n\tpath = deps/lib\n\turl = https://example.invalid/lib.git\n');
  write('src/app.mjs', 'export const value = 1;\n');
  write('tests/consumer.test.mjs', check('value === 1'));
  git('init', '-q');
  git('add', '.gitmodules', 'src', 'tests');
  git('commit', '-q', '-m', 'base');
  // Initialized submodule content exists only in this checkout, as with a fresh worktree's empty submodule.
  write('deps/lib/index.mjs', 'export const ready = true;\n');
  write('src/app.mjs', 'export const value = 2;\n');
  const tools = await createTools({ worktree, allowedFiles: ['src/app.mjs'] });
  const full = await tools.run_test({}, { full: true });
  assert.deepEqual(full.regression_files, ['tests/consumer.test.mjs']);
  assert.deepEqual(full.preexisting_files, []);
  assert.equal(readFileSync(path.join(worktree, 'deps', 'lib', 'index.mjs'), 'utf8'), 'export const ready = true;\n');
});

test('final full-suite verification separates regressions this change caused from failures already at base', async (context) => {
  const worktree = fixture(context);
  const git = (...args) => execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.com', ...args],
    { cwd: worktree, encoding: 'utf8', stdio: 'pipe' });
  mkdirSync(path.join(worktree, 'tests'));
  const write = (file, text) => writeFileSync(path.join(worktree, ...file.split('/')), text);
  const check = (expression) => "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    `import { value } from '../src/app.mjs';\ntest('check', () => assert.ok(${expression}));\n`;
  write('src/app.mjs', 'export const value = 1;\n');
  write('tests/app.test.mjs', check('value === 1'));
  write('tests/consumer.test.mjs', check('value === 1'));
  write('tests/broken.test.mjs', check('value === 99'));
  git('init', '-q');
  git('add', 'src', 'tests');
  git('commit', '-q', '-m', 'base');
  write('src/app.mjs', 'export const value = 2;\n');
  write('tests/app.test.mjs', check('value === 2'));
  const tools = await createTools({ worktree, allowedFiles: ['src/app.mjs', 'tests/app.test.mjs'] });
  assert.equal((await tools.run_test()).exit_code, 0, 'the targeted slice test alone passes');
  const full = await tools.run_test({}, { full: true });
  assert.notEqual(full.exit_code, 0);
  assert.deepEqual(full.regression_files, ['tests/consumer.test.mjs']);
  assert.deepEqual(full.preexisting_files, ['tests/broken.test.mjs']);
  assert.equal(git('worktree', 'list').trim().split('\n').length, 1, 'the base worktree is removed');
  const targeted = await tools.run_test();
  assert.notEqual(targeted.exit_code, 0, 'later targeted runs include the regressed test');
  await tools.write_file({ path: 'tests/consumer.test.mjs', content: check('value === 2') });
});

test('a full-suite failure that passes when rerun alone is transient, not pre-existing', async (context) => {
  const worktree = fixture(context);
  mkdirSync(path.join(worktree, 'tests'));
  writeFileSync(path.join(worktree, 'src', 'app.mjs'), 'export const value = 1;\n');
  // Fails only on its first run, standing in for a timeout under full-suite load.
  writeFileSync(path.join(worktree, 'tests', 'flaky.test.mjs'), "import test from 'node:test';\n" +
    "import { existsSync, writeFileSync } from 'node:fs';\n" +
    "test('flaky', () => { if (!existsSync('flaky.marker')) { writeFileSync('flaky.marker', ''); throw new Error('load'); } });\n");
  const tools = await createTools({ worktree, allowedFiles: ['src/app.mjs'] });
  const full = await tools.run_test({}, { full: true });
  assert.equal(full.exit_code, 0);
  assert.equal(full.full_suite_exit_code, 1);
  assert.deepEqual(full.transient_files, ['tests/flaky.test.mjs']);
  assert.equal(full.failing_files, undefined);
  assert.match(full.stderr, /tests\/flaky\.test\.mjs passed when rerun alone/);
});

test('a killed test process with numeric exit 1 is a terminal timeout, not a repairable check', async (context) => {
  const worktree = fixture(context);
  docsCheck(worktree);
  const tools = await createTools({ worktree, allowedFiles: ['src/repl.mjs'],
    runCommand: async () => { throw Object.assign(new Error('killed'), {
      code: 1, killed: true, stdout: 'not ok', stderr: '',
    }); } });
  await assert.rejects(tools.run_test(), /timed out after 60 seconds/);
});

test('edit_file tolerates indentation drift when unambiguous and otherwise shows the closest real text', async (context) => {
  const worktree = fixture(context);
  const file = path.join(worktree, 'src', 'shape.mjs');
  writeFileSync(file, 'export function shape() {\n    const width = 1;\n    return width;\n}\n' +
    'export function other() {\n  return 2;\n}\n');
  const tools = await createTools({ worktree, allowedFiles: ['src/shape.mjs'] });
  await tools.edit_file({ path: 'src/shape.mjs',
    old_string: '  const width = 1;\n  return width;', new_string: '  const width = 3;\n  return width * 2;' });
  assert.equal(readFileSync(file, 'utf8'), 'export function shape() {\n    const width = 3;\n    return width * 2;\n}\n' +
    'export function other() {\n  return 2;\n}\n');
  await assert.rejects(tools.edit_file({ path: 'src/shape.mjs',
    old_string: 'export function other() {\n  return 9;\n}', new_string: 'x' }),
  (error) => /not found/.test(error.message) &&
    /Closest current text starts at line 5; copy it exactly:\nexport function other\(\) \{\n  return 2;/.test(error.message));
  writeFileSync(file, 'a();\n  x();\nb();\n    x();\n');
  await assert.rejects(tools.edit_file({ path: 'src/shape.mjs', old_string: 'x();\n', new_string: 'y();' }), /more than once/);
});

test('tiered scope records capped coder expansion while hard-deny surfaces stay fatal', async (context) => {
  const worktree = fixture(context);
  writeFileSync(path.join(worktree, 'src', 'cli.mjs'), 'export const cli = 1;\n');
  const events = [];
  const tools = await createTools({ worktree, allowedFiles: ['README.md'], scopeExpansion: 2,
    onEvent: (event) => events.push(event) });
  const written = await tools.write_file({ path: 'src/state.mjs', content: 'export const state = 1;\n' });
  assert.equal(written.scope_expanded, true);
  assert.match(written.note, /outside planned TASK\.md scope \(1 of 2/);
  await assert.rejects(tools.edit_file({ path: 'src/cli.mjs', old_string: 'missing', new_string: 'x' }), /not found/);
  const edited = await tools.edit_file({ path: 'src/cli.mjs', old_string: 'cli = 1', new_string: 'cli = 2' });
  assert.equal(edited.scope_expanded, true);
  assert.equal((await tools.write_file({ path: 'src/state.mjs', content: 'export const state = 2;\n' })).scope_expanded, true);
  await assert.rejects(tools.write_file({ path: 'src/third.mjs', content: '' }), /Scope expansion limit \(2 files/);
  assert.equal(existsSync(path.join(worktree, 'src', 'third.mjs')), false);
  for (const file of ['.env', 'key.pem', '.github/workflows/build.yml', '.git/config', 'agent-policy.yml',
    'TASK.md', 'RESULT.md', '.roster/memory/coder.jsonl', 'vendor/x.mjs', '../outside.mjs']) {
    await assert.rejects(tools.write_file({ path: file, content: 'bad' }), /relative|outside|secret|not allowed|refused/i, file);
  }
  assert.deepEqual(events.filter(({ type }) => type === 'scope-expansion').map(({ path: file }) => file),
    ['src/state.mjs', 'src/cli.mjs']);
  assert.deepEqual(events.filter(({ type }) => type === 'scope-limit').map(({ path: file, limit }) => [file, limit]),
    [['src/third.mjs', 2]]);
  const strict = await createTools({ worktree, allowedFiles: ['README.md'] });
  await assert.rejects(strict.write_file({ path: 'src/other.mjs', content: '' }), /not allowed by TASK\.md/);
  const resumed = await createTools({ worktree, allowedFiles: ['README.md'], scopeExpansion: 2,
    initialScopeFiles: ['src/state.mjs', 'src/cli.mjs'] });
  assert.equal((await resumed.write_file({ path: 'src/state.mjs', content: '' })).scope_expanded, true);
  await assert.rejects(resumed.write_file({ path: 'src/fourth.mjs', content: '' }), /Scope expansion limit/);
  await assert.rejects(createTools({ worktree, seat: 'planner', scopeExpansion: 1 }), /coder seat/);
  const repairing = await createTools({ worktree, allowedFiles: ['README.md'], initialRepairFiles: ['tests/state.test.mjs'] });
  assert.equal((await repairing.write_file({ path: 'tests/state.test.mjs', content: '' })).scope_expanded, undefined);
  await assert.rejects(createTools({ worktree, allowedFiles: ['README.md'], initialRepairFiles: ['src/state.mjs'] }),
    /Repair scope/);
  if (process.platform === 'win32') {
    const variant = await resumed.write_file({ path: 'SRC/STATE.MJS', content: '' });
    assert.equal(variant.scope_path, 'src/state.mjs');
  }
  mkdirSync(path.join(worktree, 'tests'), { recursive: true });
  writeFileSync(path.join(worktree, 'tests', 'state.test.mjs'), '');
  let testArgs;
  const testing = await createTools({ worktree, allowedFiles: ['README.md'], scopeExpansion: 1,
    initialScopeFiles: ['src/state.mjs'], runCommand: async (_program, args) => {
      testArgs = args;
      return { stdout: 'ok', stderr: '' };
    } });
  await testing.run_test({});
  assert.ok(testArgs.includes('tests/state.test.mjs'), 'expanded source files run their related tests');
  assert.throws(() => taskAndRepairFiles(['README.md'], [], ['.env']), /Scope expansion/);
  assert.deepEqual(taskAndRepairFiles(['README.md'], [], ['src/a.mjs']), ['README.md', 'src/a.mjs']);
});

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
    '.git/config', 'TASK.md', 'ASSIGNMENT.md', 'RESULT.md', 'REVIEW.md',
    'RECIPE.yml', 'ESTIMATE.md', '.roster/evals.jsonl']) {
    await assert.rejects(tools.write_file({ path: file, content: 'bad' }), /relative|inside|outside the worktree|secret|not allowed/i, file);
  }
  await assert.rejects(tools.read_file({ path: '.env' }), /secrets/);
  await assert.rejects(tools.read_file({ path: '../outside.md' }), /Refused: outside the worktree/);
  await assert.rejects(tools.write_file({ path: 'README.md', content: 42 }), /must be text/);
  await assert.rejects(tools.read_file({ path: 'README.md', ignored: true }), /Tool arguments/);
  assert.equal(isForbiddenWrite('other/.github/workflows/ci.yml'), true);
  assert.equal(isAllowedFile('src/other.mjs', ['src/**']), true);
  assert.equal(isAllowedFile('docs/file.md', ['src/**']), false);
  const broad = await createTools({ worktree, allowedFiles: ['**/*'] });
  await assert.rejects(broad.write_file({ path: '.roster/runs/session.log', content: 'forged' }), /not allowed/);
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
    ['.github', 'README.md', 'src']);
  assert.deepEqual((await tools.list_dir({ path: '.github' })).map(({ name }) => name), []);
  await assert.rejects(tools.list_dir({ path: 'vendor' }), /Refused: outside the worktree/);
  assert.deepEqual((await tools.list_dir({ path: 'src' })).map(({ name }) => name), []);
  for (const file of ['..', path.dirname(worktree), '.env', 'agent-policy.yml',
    '.github/workflows', 'src/key.pem', 'src/.env.private', 'src/agent-policy.yml',
    'vendor/github-agent-contracts']) {
    await assert.rejects(tools.list_dir({ path: file }),
      /relative|inside|outside the worktree|secrets|Listing protected/i, file);
  }
  for (const file of ['agent-policy.yml', '.github/workflows/ci.yml',
    'vendor/github-agent-contracts/checker.mjs', 'src/key.pem', 'src/agent-policy.yml']) {
    await assert.rejects(tools.write_file({ path: file, content: 'changed' }), /not allowed|secrets|outside the worktree/);
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
  const directoryLink = path.join(worktree, 'src', 'outside');
  writeFileSync(outside, 'outside');
  try {
    symlinkSync(outside, link);
    symlinkSync(outsideDirectory, directoryLink, process.platform === 'win32' ? 'junction' : 'dir');
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
  await assert.rejects(tools.write_file({ path: 'src/outside/created/new.mjs', content: 'bad' }), /symlinks/);
  assert.equal(existsSync(path.join(outsideDirectory, 'created')), false);
  assert.equal(readFileSync(outside, 'utf8'), 'outside');
});

test('a README-only task skips node tests and does not spawn a suite', async (context) => {
  const worktree = fixture(context);
  docsCheck(worktree);
  let called = false;
  const tools = await createTools({
    worktree, allowedFiles: ['README.md'], apiKeyEnv: 'CUSTOM_KEY',
    env: { PATH: process.env.PATH, CUSTOM_KEY: 'secret', GH_TOKEN: 'token',
      NODE_TEST_CONTEXT: 'child-v8',
      GITHUB_APP_ID: '1', GITHUB_APP_PRIVATE_KEY_PATH: 'key.pem' },
    runCommand: async () => { called = true; return { stdout: 'tests pass', stderr: '' }; },
  });
  assert.deepEqual(await tools.run_test({}), {
    exit_code: 0, skipped: true, stdout: 'docs-only: tests skipped', stderr: '',
  });
  assert.equal(called, false);
  await assert.rejects(tools.run_test({ command: 'echo secret' }), /Tool arguments/);
});

test('a code slice runs only the matching test file', async (context) => {
  const worktree = fixture(context);
  docsCheck(worktree);
  let received;
  const tools = await createTools({ worktree, allowedFiles: ['src/repl.mjs'],
    runCommand: async (_program, args, options) => { received = { args, timeout: options.timeout }; return { stdout: '', stderr: '' }; } });
  await tools.run_test();
  assert.deepEqual(received.args.slice(0, 3), ['--test', '--test-concurrency', received.args[2]]);
  assert.equal(received.args[3], '--test-timeout=20000');
  assert.equal(received.args.at(-1), 'tests/repl.test.mjs');
  assert.equal(received.timeout, 60_000);
  const otherRepo = fixture(context);
  const fallback = await createTools({ worktree: otherRepo, allowedFiles: ['README.md'],
    runCommand: async () => { throw new Error('docs-only must not spawn'); } });
  assert.equal((await fallback.run_test()).skipped, true);
});

test('every tool refuses "..", vendor, and absolute paths before it runs', async (context) => {
  const worktree = fixture(context);
  mkdirSync(path.join(worktree, 'vendor', 'github-agent-contracts', 'scripts'), { recursive: true });
  writeFileSync(path.join(worktree, 'vendor', 'github-agent-contracts', 'scripts', 'agent-pr.mjs'), '');
  const events = [];
  for (const options of [{ allowedFiles: ['**/*'] }, { allowedFiles: ['README.md'], readmeOnlyDocs: true },
    { allowedFiles: ['**/*'], onEvent: async (event) => { events.push(event); } }]) {
    const tools = await createTools({ worktree, ...options });
    for (const target of ['..', '../outside', 'src/../../x', 'vendor', './vendor/github-agent-contracts',
      'Vendor\\x', path.join(worktree, 'README.md'), 'C:/Windows']) {
      await assert.rejects(tools.list_dir({ path: target }), /Refused: outside the worktree\.$/);
      await assert.rejects(tools.read_file({ path: target }), /Refused: outside the worktree\.$/);
      await assert.rejects(tools.write_file({ path: target, content: 'x' }), /Refused: outside the worktree\.$/);
      await assert.rejects(tools.search_text({ path: target, query: 'x' }), /Refused: outside the worktree\.$/);
    }
  }
  assert.ok(events.length > 0);
  assert.ok(events.every((event) => event.type !== 'tool'), 'a refused path must not start the tool');
  assert.ok(events.some((event) => event.type === 'tool-refused' && event.name === 'list_dir'));
  const listed = await (await createTools({ worktree, allowedFiles: ['**/*'] })).list_dir({ path: '.' });
  assert.ok(!listed.some(({ name }) => name === 'vendor'), 'vendor must not appear in a listing');
});

test('empty and multiline searches are denied before a tool-start event', async (context) => {
  const worktree = fixture(context);
  const events = [];
  const tools = await createTools({
    worktree,
    allowedFiles: ['README.md'],
    onEvent: async (event) => { events.push(event); },
  });
  for (const query of ['', 'Status\nPrivate']) {
    await assert.rejects(tools.search_text({ query, path: 'README.md' }),
      /search_text query must be nonempty, single-line literal text/);
  }
  assert.ok(!events.some((event) => event.type === 'tool' && event.name === 'search_text'));
  assert.equal(events.filter((event) =>
    event.type === 'tool-result' && event.name === 'search_text' && event.status === 'denied').length, 2);
});

test('a disabled run_test permission refuses execution even for direct harness calls', async (context) => {
  const worktree = fixture(context);
  const tools = await createTools({
    worktree, allowedFiles: ['README.md'], allowRunTest: false,
    runCommand: () => assert.fail('A denied test must not start a child process'),
  });
  await assert.rejects(tools.run_test({}), /run_test is disabled by tools\.run_test/);
  assert.equal(await tools.read_file({ path: 'README.md' }), '# Example\n');
  await assert.rejects(createTools({ worktree, allowedFiles: ['README.md'], allowRunTest: 'no' }),
    /permission must be a boolean/);
});

test('run_test actually executes Node tests from the worktree', async (context) => {
  const worktree = fixture(context);
  writeFileSync(path.join(worktree, 'example.test.mjs'),
    "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    "import { writeFileSync } from 'node:fs';\n" +
    "test('example', () => { assert.equal(2 + 2, 4); writeFileSync('ran-marker.txt', 'yes'); });\n");
  const tools = await createTools({ worktree, allowedFiles: ['example.test.mjs'] });
  const result = await tools.run_test();
  assert.equal(result.exit_code, 0, result.stderr);
  assert.equal(readFileSync(path.join(worktree, 'ran-marker.txt'), 'utf8'), 'yes');
});

test('coder tools deny protected reads and Windows alias or stream paths', async (context) => {
  const worktree = fixture(context);
  const tools = await createTools({ worktree, allowedFiles: ['**/*'] });
  const names = ['read_file', 'write_file', 'edit_file', 'delete_file', 'glob_files', 'run_command', 'list_dir', 'run_test', 'search_text', 'web_search', 'web_fetch'];
  assert.deepEqual(Object.keys(tools), names);
  assert.deepEqual(toolDefinitions.map(({ function: tool }) => tool.name), names);
  for (const file of ['.env', 'nested/.env.local', 'production.env', 'key.pem',
    'agent-policy.yml', '.github/workflows/build.yml', '.git/config',
    '.roster/vault/.key', 'vendor/github-agent-contracts/scripts/agent-pr.mjs']) {
    assert.equal(isForbiddenRead(file), true, file);
    await assert.rejects(tools.read_file({ path: file }), /secrets|outside the worktree/, file);
    await assert.rejects(tools.search_text({ query: 'secret', path: file }), /secrets|outside the worktree/, file);
  }
  for (const file of ['.env ', 'key.pem.', 'key.pem::$DATA', '.git:metadata',
    'src/agent-policy.yml.', 'src/file.mjs:stream']) {
    await assert.rejects(tools.read_file({ path: file }), /ambiguous Windows|data streams/, file);
    await assert.rejects(tools.write_file({ path: file, content: 'bad' }),
      /ambiguous Windows|data streams/, file);
  }
  await assert.rejects(tools.search_text({ query: 'secret', path: '../outside' }), /Refused: outside the worktree/);
});

test('literal search caps results at fifty lines and never shells out or reveals protected bodies', async (context) => {
  const worktree = fixture(context);
  const marker = 'literal.*marker';
  writeFileSync(path.join(worktree, 'src', 'matches.txt'),
    Array.from({ length: 51 }, (_, index) => `${marker} ${index + 1}`).join('\n'));
  writeFileSync(path.join(worktree, 'src', 'binary.bin'), `${marker}\0hidden`);
  for (const file of ['.env', 'agent-policy.yml', 'secret.pem', 'production.env']) {
    writeFileSync(path.join(worktree, file), marker);
  }
  mkdirSync(path.join(worktree, '.github', 'workflows'), { recursive: true });
  writeFileSync(path.join(worktree, '.github', 'workflows', 'ci.yml'), marker);
  const tools = await createTools({ worktree, allowedFiles: ['README.md'],
    runCommand: () => { throw new Error('search_text must not start a subprocess'); } });
  const result = await tools.search_text({ query: marker });
  assert.equal(result.matches.length, 50);
  assert.equal(result.truncated, true);
  assert.ok(result.matches.every(({ path: file }) => file === 'src/matches.txt'));
  assert.deepEqual(result.matches.map(({ line }) => line),
    Array.from({ length: 50 }, (_, index) => index + 1));
  assert.equal(result.matches[0].text, `${marker} 1`);
  writeFileSync(path.join(worktree, 'src', 'matches.txt'), `${marker}\n`.repeat(50));
  const exact = await tools.search_text({ query: marker, path: 'src/matches.txt' });
  assert.equal(exact.matches.length, 50);
  assert.equal(exact.truncated, false);
  assert.deepEqual(await tools.search_text({ query: 'literal.+marker', path: 'src' }),
    { matches: [], truncated: false });
  await assert.rejects(tools.search_text({ query: '' }), /nonempty/);
  await assert.rejects(tools.search_text({ query: 'one\ntwo' }), /single-line/);
  await assert.rejects(tools.search_text({ query: marker, command: 'arbitrary' }), /Tool arguments/);
});

test('read_file limits research excerpts without changing ordinary reads', async (context) => {
  const worktree = fixture(context);
  writeFileSync(path.join(worktree, 'README.md'), 'first\r\nsecond\r\nthird\r\n');
  const tools = await createTools({ worktree, allowedFiles: ['README.md'] });
  assert.equal(await tools.read_file({ path: 'README.md', max_lines: 2 }), 'first\nsecond');
  assert.equal(await tools.read_file({ path: 'README.md' }), 'first\r\nsecond\r\nthird\r\n');
  for (const max_lines of [0, -1, 1.5, '2', null]) {
    await assert.rejects(tools.read_file({ path: 'README.md', max_lines }), /positive safe integer/);
  }
});

test('delete_file removes scope and untracked scratch files but refuses tracked out-of-scope and protected paths', async (context) => {
  const worktree = fixture(context);
  const { execFileSync } = await import('node:child_process');
  const git = (...args) => execFileSync('git', args, { cwd: worktree, stdio: 'pipe' });
  git('init', '-q');
  writeFileSync(path.join(worktree, 'src', 'app.mjs'), 'export const a = 1;\n');
  writeFileSync(path.join(worktree, 'src', 'other.mjs'), 'export const b = 2;\n');
  git('add', 'README.md', 'src/app.mjs', 'src/other.mjs');
  writeFileSync(path.join(worktree, 'probe.mjs'), 'console.log(1);\n');
  writeFileSync(path.join(worktree, 'TASK.md'), 'Task\n');
  const tools = await createTools({ worktree, allowedFiles: ['src/app.mjs'] });
  assert.deepEqual(await tools.delete_file({ path: 'probe.mjs' }), { path: 'probe.mjs', deleted: true });
  assert.equal(existsSync(path.join(worktree, 'probe.mjs')), false);
  assert.deepEqual(await tools.delete_file({ path: 'src/app.mjs' }), { path: 'src/app.mjs', deleted: true });
  await assert.rejects(tools.delete_file({ path: 'src/other.mjs' }), /Git tracks it and it is outside TASK\.md scope/);
  assert.equal(existsSync(path.join(worktree, 'src', 'other.mjs')), true);
  for (const file of ['.env', 'TASK.md', '.roster/runs/x.log', 'vendor/x.mjs']) {
    await assert.rejects(tools.delete_file({ path: file }), /protected, harness, and private paths|outside the worktree/, file);
  }
  await assert.rejects(tools.delete_file({ path: '../outside.mjs' }), /outside the worktree/);
  await assert.rejects(tools.delete_file({ path: 'missing.mjs' }), /does not exist/);
  assert.equal(existsSync(path.join(worktree, '.env')), true);
});

test('a targeted run includes every shard of a split module test', async (context) => {
  const worktree = fixture(context);
  docsCheck(worktree);
  for (const name of ['builtin.models.test.mjs', 'builtin.resume.test.mjs', 'builtin-other.test.mjs']) {
    writeFileSync(path.join(worktree, 'tests', name), '');
  }
  let received;
  const tools = await createTools({ worktree, allowedFiles: ['src/lib/builtin.mjs'],
    runCommand: async (_program, args) => { received = args; return { stdout: '', stderr: '' }; } });
  await tools.run_test();
  assert.deepEqual(received.slice(4), ['tests/builtin.models.test.mjs', 'tests/builtin.resume.test.mjs']);
});

test('a coder write to a harness report is a recoverable usage denial; handoff tampering stays a hard stop', async (t) => {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-report-write-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  const tools = await createTools({ worktree, allowedFiles: ['src/app.mjs'] });
  for (const file of ['RESULT.md', 'REVIEW.md']) {
    await assert.rejects(tools.write_file({ path: file, content: 'forged' }), (error) => {
      assert.equal(error.constructor.name, 'ToolUsageError');
      assert.match(error.message, new RegExp(`Writing ${file.replace('.', '\\.')} is not allowed: the harness writes it`));
      return true;
    });
    assert.equal(existsSync(path.join(worktree, file)), false);
  }
  await assert.rejects(tools.write_file({ path: 'TASK.md', content: 'forged' }), (error) =>
    error.constructor.name === 'ToolAccessError' && /Writing TASK\.md is not allowed/.test(error.message));
});
