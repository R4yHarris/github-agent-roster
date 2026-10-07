import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createRunLog, readLastRunLog } from '../src/lib/run-log.mjs';
import { taskTestsMode } from '../src/runtime/excellence.mjs';
import { checkRedGreen, newTestNames, redGreenTable, tapResults, testNames } from '../src/runtime/red-green.mjs';
import { runCoder as runCoderSeat } from '../src/seats/coder.mjs';
import { withResearchSummary } from './helpers/research.mjs';

const git = (cwd, ...args) => execFileSync('git', ['-c', 'core.autocrlf=false', '-c', 'user.email=t@example.com', '-c', 'user.name=t', ...args],
  { cwd, encoding: 'utf8' });
const baseTest = "import assert from 'node:assert/strict';\nimport test from 'node:test';\n" +
  "import { add } from '../src/math.mjs';\n\ntest('adds', () => assert.equal(add(1, 2), 3));\n";

function repo(context) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-red-green-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true }));
  mkdirSync(path.join(worktree, 'src'));
  mkdirSync(path.join(worktree, 'tests'));
  writeFileSync(path.join(worktree, 'package.json'), '{ "type": "module" }\n');
  writeFileSync(path.join(worktree, 'src', 'math.mjs'), 'export const add = (a, b) => a + b;\n');
  writeFileSync(path.join(worktree, 'tests', 'math.test.mjs'), baseTest);
  git(worktree, 'init', '-q');
  git(worktree, 'add', '-A');
  git(worktree, 'commit', '-q', '-m', 'base');
  return worktree;
}

test('new test names come from literal test() calls absent at base; TAP results map names to pass', () => {
  assert.deepEqual(testNames("test('a', f); it(\"b \\\"q\\\"\", f); test(`t${x}`, f);"), ['a', 'b "q"']);
  assert.deepEqual(newTestNames("test('a', f); test('b', f);", "test('a', f);"), ['b']);
  assert.deepEqual(newTestNames("test('a', f);", null), ['a']);
  const results = tapResults('ok 1 - a\nnot ok 2 - b\n    ok 1 - nested \\# hash\nok 3 - c # SKIP later\n');
  assert.deepEqual([...results], [['a', true], ['b', false], ['nested # hash', true], ['c', false]]);
});

test('a new test that passes on base is not red; one that fails on base and passes on the candidate is red/green', async (context) => {
  const worktree = repo(context);
  writeFileSync(path.join(worktree, 'src', 'math.mjs'),
    'export const add = (a, b) => a + b;\nexport const sub = (a, b) => a - b;\n');
  writeFileSync(path.join(worktree, 'tests', 'math.test.mjs'), baseTest +
    "test('adds zero', () => assert.equal(add(2, 0), 2));\n");
  writeFileSync(path.join(worktree, 'tests', 'sub.test.mjs'), "import assert from 'node:assert/strict';\n" +
    "import test from 'node:test';\nimport * as math from '../src/math.mjs';\n\n" +
    "test('subtracts', () => assert.equal(math.sub(3, 1), 2));\n");
  const status = git(worktree, 'status', '--porcelain');
  const result = await checkRedGreen({ worktree, files: ['src/math.mjs', 'tests/math.test.mjs', 'tests/sub.test.mjs'] });
  assert.equal(result.status, 'checked');
  assert.deepEqual(result.tests, [
    { file: 'tests/math.test.mjs', name: 'adds zero', base: 'pass', candidate: 'pass' },
    { file: 'tests/sub.test.mjs', name: 'subtracts', base: 'fail', candidate: 'pass' },
  ]);
  assert.deepEqual(result.notRed.map(({ name }) => name), ['adds zero']);
  assert.equal(git(worktree, 'status', '--porcelain'), status, 'the gate never edits the worktree');
  assert.equal(git(worktree, 'worktree', 'list').trim().split('\n').length, 1, 'the base worktree is removed');
  assert.match(redGreenTable(result), /\| tests\/sub\.test\.mjs \| subtracts \| fail \| pass \|/);
});

test('characterization and waived tasks are exempt; unchanged tests and non-git worktrees run nothing', async (context) => {
  const worktree = repo(context);
  writeFileSync(path.join(worktree, 'tests', 'math.test.mjs'), baseTest + "test('adds zero', () => assert.equal(add(2, 0), 2));\n");
  const files = ['tests/math.test.mjs'];
  assert.equal(taskTestsMode('---\ntests: characterization\n---\n# Task\n'), 'characterization');
  assert.equal((await checkRedGreen({ worktree, files, mode: 'characterization' })).status, 'exempt');
  assert.equal((await checkRedGreen({ worktree, files, mode: 'none' })).status, 'skipped');
  assert.equal((await checkRedGreen({ worktree, files: ['src/math.mjs'] })).status, 'none');
  assert.deepEqual(await checkRedGreen({ worktree, files }),
    { status: 'exempt', reason: 'test-only change', tests: [], notRed: [] });
  const plain = mkdtempSync(path.join(tmpdir(), 'roster-red-green-plain-'));
  context.after(() => rmSync(plain, { recursive: true, force: true }));
  mkdirSync(path.join(plain, 'tests'));
  writeFileSync(path.join(plain, 'tests', 'x.test.mjs'), "test('x', () => {});\n");
  assert.equal((await checkRedGreen({ worktree: plain, files: ['src/x.mjs', 'tests/x.test.mjs'] })).status, 'unavailable');
  assert.match(redGreenTable({ status: 'exempt', tests: [] }), /^Red\/green: exempt/);
});

test('the coder gets one not-red correction, and RESULT.md records the red/green evidence', async (context) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-red-green-coder-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'),
    readFileSync(new URL('../principals/coder.md', import.meta.url), 'utf8'));
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  const worktree = repo(context);
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nCode carefully.\n');
  writeFileSync(path.join(worktree, 'TASK.md'), '---\nreference: issue:5\ntask_class: feat\ndifficulty: 3\n---\n' +
    '# Task\n\n## Ask\nAdd `sub` to `src/math.mjs`.\n\n## Acceptance checks\n1. `sub(3, 1)` returns 2.\n\n' +
    '## Files allowed\n- src/math.mjs\n- tests/math.test.mjs\n');
  git(worktree, 'add', '-A');
  git(worktree, 'commit', '-q', '-m', 'task');
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
    .replace('base_url: ""', 'base_url: http://localhost:3456/v1').replace('model: ""', 'model: local-model')
    .replace('turn_budget: 1000', 'turn_budget: 4'));
  const write = (id, content) => ({ id, type: 'function', function: { name: 'write_file',
    arguments: JSON.stringify({ path: 'tests/math.test.mjs', content }) } });
  const reply = (message) => ({ status: 200, json: async () => ({ choices: [{
    finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message: { role: 'assistant', content: null, ...message } }],
  usage: { prompt_tokens: 5, completion_tokens: 2 } }) });
  const sources = 'export const add = (a, b) => a + b;\nexport const sub = (a, b) => a - b;\n';
  const subTest = baseTest.replace('import { add }', 'import { add, sub }') +
    "test('subtracts', () => assert.equal(sub(3, 1), 2));\n";
  let calls = 0;
  let correction;
  const events = [];
  const fetchImpl = async (_url, request) => {
    calls += 1;
    const body = JSON.parse(request.body);
    if (calls === 1) {
      return reply({ tool_calls: [
        { id: 'r1', type: 'function', function: { name: 'read_file', arguments: JSON.stringify({ path: 'src/math.mjs' }) } },
        { id: 'r2', type: 'function', function: { name: 'read_file', arguments: JSON.stringify({ path: 'tests/math.test.mjs' }) } },
        { id: 's1', type: 'function', function: { name: 'write_file', arguments: JSON.stringify({ path: 'src/math.mjs', content: sources }) } },
        write('t1', baseTest + "test('adds zero', () => assert.equal(add(2, 0), 2));\n"),
      ] });
    }
    if (calls === 2) return reply({ content: 'Added sub with a test.' });
    if (calls === 3) {
      correction = body.messages.at(-1).content;
      return reply({ tool_calls: [write('t2', subTest)] });
    }
    return reply({ content: 'Added sub; the new test fails without it.' });
  };
  const result = await runCoderSeat({ onEvent: async (event) => { if (event.type === 'red-green') events.push(event); }, repoRoot, worktree, config, task: 'issue-5', session: 'roster-session',
    memoryPath: path.join(repoRoot, config.paths.memory), env: {}, fetchImpl: withResearchSummary(fetchImpl) });
  assert.deepEqual(events.map(({ notRed }) => notRed), [1, 0]);
  assert.match(correction, /^Not red: "adds zero" \(tests\/math\.test\.mjs\)/);
  assert.match(correction, /One red\/green correction is allowed/);
  assert.equal(calls, 4);
  assert.deepEqual(result.redGreen.notRed, []);
  assert.deepEqual(result.redGreen.tests, [{ file: 'tests/math.test.mjs', name: 'subtracts', base: 'fail', candidate: 'pass' }]);
  assert.match(readFileSync(path.join(worktree, 'RESULT.md'), 'utf8'),
    /## Red\/green\n\n\| Test file \| Test \| Base \| Candidate \|\n\|---\|---\|---\|---\|\n\| tests\/math\.test\.mjs \| subtracts \| fail \| pass \|/);
});

test('red/green results reach the run log', async (context) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-red-green-log-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  let text = '';
  const options = { repoRoot, session: 'roster-5-coder', env: {}, errorOutput: { write(value) { text += value; } } };
  const logger = await createRunLog(options);
  await logger.seat('coder', options.session, { llm: { base_url: '', model: '' } }, async (onEvent) => {
    await onEvent({ type: 'red-green', status: 'checked', tests: 2, notRed: 1 });
    assert.match((await readLastRunLog(options)).lastLine, /seat coder red-green checked tests=2 not-red=1$/);
  });
  assert.match(text, /1 new tests already pass on the base revision \(not red\)\./);
});
