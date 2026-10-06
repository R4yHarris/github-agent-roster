import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { assertContractsInitialized, ContractsSubmoduleError, initializeWorktreeSubmodules,
  onlyMissingContractsScripts } from '../src/lib/contracts.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { runLoop } from '../src/runtime/loop.mjs';
import { createTools } from '../src/runtime/tools.mjs';
import { planStub } from '../src/planner/stub.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: test-model'));
const missing = "Error [ERR_MODULE_NOT_FOUND]: Cannot find module '/repo/vendor/github-agent-contracts/scripts/agent-pr.mjs' imported from /repo/tests/example.test.mjs";

function fixture(t) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-contracts-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  return worktree;
}

test('worktree initialization runs recursive submodule update at its root and verifies a declared publisher', async (t) => {
  const worktree = fixture(t);
  writeFileSync(path.join(worktree, '.gitmodules'),
    '[submodule "github-agent-contracts"]\n\tpath = vendor/github-agent-contracts\n\turl = fixture\n');
  await assert.rejects(assertContractsInitialized(worktree), ContractsSubmoduleError);
  const calls = [];
  await initializeWorktreeSubmodules(worktree, async (program, args, cwd) => {
    calls.push([program, args, cwd]);
    const scripts = path.join(worktree, 'vendor', 'github-agent-contracts', 'scripts');
    mkdirSync(scripts, { recursive: true });
    writeFileSync(path.join(scripts, 'agent-pr.mjs'), 'export {};\n');
    return '';
  });
  assert.deepEqual(calls, [['git', ['submodule', 'update', '--init', '--recursive'], worktree]]);
  await assertContractsInitialized(worktree);
});

test('worktree initialization skips submodule update when the publisher is already present', async (t) => {
  const worktree = fixture(t);
  const publisher = path.join(worktree, 'vendor', 'github-agent-contracts', 'scripts', 'agent-pr.mjs');
  mkdirSync(path.dirname(publisher), { recursive: true });
  writeFileSync(publisher, 'export {};\n');
  const calls = [];
  const initialized = await initializeWorktreeSubmodules(worktree, async (...args) => calls.push(args));
  assert.equal(initialized, false);
  assert.deepEqual(calls, []);
});

test('worktree submodule initialization executes at most once when no publisher is declared', async (t) => {
  const worktree = fixture(t);
  writeFileSync(path.join(worktree, '.gitmodules'), '[submodule "other"]\n\tpath = vendor/other\n\turl = fixture\n');
  let calls = 0;
  const runCommand = async () => { calls += 1; return ''; };
  assert.equal(await initializeWorktreeSubmodules(worktree, runCommand), true);
  assert.equal(await initializeWorktreeSubmodules(worktree, runCommand), false);
  assert.equal(calls, 1);
});

test('worktree initialization skips git submodule when the worktree declares no submodules', async (t) => {
  const worktree = fixture(t);
  const runCommand = async () => assert.fail('a worktree without .gitmodules has nothing to initialize');
  assert.equal(await initializeWorktreeSubmodules(worktree, runCommand), false);
});

test('dependency-only errors are distinguished from real or mixed test failures', () => {
  assert.equal(onlyMissingContractsScripts({ exit_code: 1, stderr: missing }), true);
  assert.equal(onlyMissingContractsScripts({ exit_code: 1, stderr: missing.replaceAll('/', '\\\\') }), true);
  assert.equal(onlyMissingContractsScripts({ exit_code: 1, stdout: `${missing}\nnot ok 1\n# fail 1\n` }), true);
  for (const output of [
    `${missing}\nAssertionError [ERR_ASSERTION]: wrong value`,
    `${missing}\nnot ok 1\nnot ok 2\n# fail 2\n`,
    `${missing}\nerror: 'behavior was incorrect'`,
    missing.replace('/vendor/github-agent-contracts/scripts/agent-pr.mjs', '/src/app.mjs'),
    'not ok 1\nerror: missing contracts\n',
  ]) assert.equal(onlyMissingContractsScripts({ exit_code: 1, stderr: output }), false);
  assert.equal(onlyMissingContractsScripts({ exit_code: 0, stderr: missing }), false);
});

test('missing contracts blocks node tests with the short diagnostic rather than exposing vendor paths', async (t) => {
  const worktree = fixture(t);
  writeFileSync(path.join(worktree, 'example.test.mjs'), "import test from 'node:test';\n");
  const events = [];
  const tools = await createTools({ worktree, allowedFiles: ['example.test.mjs'],
    onEvent: (event) => events.push(event),
    runCommand: async () => { throw Object.assign(new Error('node tests failed'), { code: 1, stderr: missing }); },
  });
  await assert.rejects(tools.run_test(), (error) =>
    error instanceof ContractsSubmoduleError && error.message === 'Contracts submodule was not initialized' &&
    !error.message.includes('vendor/'));
  assert.ok(events.some(({ type }) => type === 'contracts-uninitialized'));
});

test('missing contracts is infrastructure-blocked and consumes no slice repair attempts', async () => {
  let turns = 0;
  const task = planStub('Update src/app.mjs.').task;
  const events = [];
  const result = await runLoop({ config, context: { task, pack: task }, env: {},
    tools: { run_test: async () => ({ exit_code: 1, stderr: missing, stdout: '' }) },
    onEvent: (event) => events.push(event),
    fetchImpl: async () => {
      turns += 1;
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }] });
    },
    verify: () => assert.fail('Uninitialized contracts cannot verify a slice'),
  });
  assert.equal(result.blocked, true);
  assert.equal(result.testRepairs, 0);
  assert.equal(turns, 1);
  assert.equal(result.summary, 'Contracts submodule was not initialized');
  assert.doesNotMatch(result.summary, /vendor[\\/]/);
  assert.ok(!events.some(({ type, name }) => type === 'test-repair' || type === 'tool' && name === 'list_dir'));
});

test('real Node missing-contracts imports block, but mixed assertion failures remain failed tests', async (t) => {
  const worktree = fixture(t);
  writeFileSync(path.join(worktree, 'dependency.test.mjs'),
    "import './vendor/github-agent-contracts/scripts/agent-pr.mjs';\n");
  const tools = await createTools({ worktree, allowedFiles: ['dependency.test.mjs', 'behavior.test.mjs'] });
  await assert.rejects(tools.run_test(), ContractsSubmoduleError);
  writeFileSync(path.join(worktree, 'behavior.test.mjs'),
    "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    "test('behavior', () => assert.equal(1, 2));\n");
  const mixed = await tools.run_test();
  assert.equal(mixed.exit_code, 1);
  assert.equal(onlyMissingContractsScripts(mixed), false);
});
