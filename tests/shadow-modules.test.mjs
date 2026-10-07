import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createRunLog, readLastRunLog } from '../src/lib/run-log.mjs';
import { checkShadowModules, exportNames, profile } from '../src/runtime/shadow-modules.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { withResearchSummary } from './helpers/research.mjs';

const git = (cwd, ...args) => execFileSync('git', ['-c', 'core.autocrlf=false', '-c', 'user.email=t@example.com', '-c', 'user.name=t', ...args],
  { cwd, encoding: 'utf8' });
const store = 'const rows = [];\nexport function readAll() {\n  return [...rows];\n}\nexport function appendRecord(record) {\n  rows.push(record);\n}\n';
const baseTest = "import assert from 'node:assert/strict';\nimport test from 'node:test';\n" +
  "import { appendRecord, readAll } from '../src/record-store.mjs';\n\n" +
  "test('appends', () => { appendRecord({ id: 1 }); assert.equal(readAll().length, 1); });\n";

function repo(context) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-shadow-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true }));
  mkdirSync(path.join(worktree, 'src'));
  mkdirSync(path.join(worktree, 'tests'));
  writeFileSync(path.join(worktree, 'package.json'), '{ "type": "module" }\n');
  writeFileSync(path.join(worktree, 'src', 'record-store.mjs'), store);
  writeFileSync(path.join(worktree, 'tests', 'store.test.mjs'), baseTest);
  git(worktree, 'init', '-q');
  git(worktree, 'add', '-A');
  git(worktree, 'commit', '-q', '-m', 'base');
  return worktree;
}

test('export names and role profiles come from source text', () => {
  assert.deepEqual(exportNames('export async function readAll() {}\nexport const x = 1;\nexport { a, b as c };\nconst y = 2;\n'),
    ['readAll', 'x', 'a', 'c']);
  assert.deepEqual(profile('readRecords'), { role: 'read', nouns: new Set(['record']) });
  assert.deepEqual(profile('appendRecord'), { role: 'write', nouns: new Set(['record']) });
  assert.deepEqual(profile('formatDuration'), { role: 'format', nouns: new Set(['duration']) });
});

test('a new readRecords file next to a readAll store is flagged; a genuine new export is not', async (context) => {
  const worktree = repo(context);
  writeFileSync(path.join(worktree, 'src', 'records.mjs'), 'export function readRecords() {\n  return [];\n}\n');
  writeFileSync(path.join(worktree, 'src', 'duration.mjs'), 'export const formatDuration = (ms) => `${ms} ms`;\n');
  const status = git(worktree, 'status', '--porcelain');
  const result = await checkShadowModules({ worktree, files: ['src/records.mjs', 'src/duration.mjs'],
    priorWaveFiles: ['src/record-store.mjs'], taskText: 'Add formatDuration and readRecords.' });
  assert.equal(result.status, 'flagged');
  assert.deepEqual(result.findings.map(({ file, name, existing }) => [file, name, existing]),
    [['src/records.mjs', 'readRecords', 'src/record-store.mjs']]);
  assert.match(result.findings[0].reason, /^Shadow module: new file src\/records\.mjs adds readRecords, which duplicates the read role of readAll in src\/record-store\.mjs \(earlier wave\); extend src\/record-store\.mjs/);
  assert.equal(git(worktree, 'status', '--porcelain'), status, 'the gate never edits the worktree');
});

test('an exact export name in another module is flagged; extending the module itself is not', async (context) => {
  const worktree = repo(context);
  writeFileSync(path.join(worktree, 'src', 'ledger.mjs'), 'export function appendRecord() {}\n');
  writeFileSync(path.join(worktree, 'src', 'record-store.mjs'), `${store}export function readRecords() {\n  return readAll();\n}\n`);
  const result = await checkShadowModules({ worktree, files: ['src/ledger.mjs', 'src/record-store.mjs'],
    taskText: 'Expose readRecords.' });
  assert.deepEqual(result.findings.map(({ name, existing }) => [name, existing]), [['appendRecord', 'src/record-store.mjs']]);
  assert.match(result.findings[0].reason, /already exports; import or extend appendRecord in src\/record-store\.mjs instead/);
});

test('new exports with no product caller are flagged unless used, task-named, or an entry point', async (context) => {
  const worktree = repo(context);
  writeFileSync(path.join(worktree, 'src', 'paths.mjs'), 'export function normalizePath(p) {\n  return p;\n}\n' +
    'export function pathsEqual(a, b) {\n  return normalizePath(a) === normalizePath(b);\n}\n' +
    'export const helperUsedInside = 1;\nexport const wired = () => helperUsedInside;\nexport const named = 2;\n');
  writeFileSync(path.join(worktree, 'src', 'main.mjs'), "import { wired } from './paths.mjs';\nexport const run = () => wired();\n");
  writeFileSync(path.join(worktree, 'src', 'cli.mjs'), 'export function main() {}\n');
  writeFileSync(path.join(worktree, 'tests', 'paths.test.mjs'), "import { pathsEqual } from '../src/paths.mjs';\npathsEqual('a', 'a');\n");
  const result = await checkShadowModules({ worktree, taskText: 'Add `named` for the next wave.',
    files: ['src/paths.mjs', 'src/main.mjs', 'src/cli.mjs', 'tests/paths.test.mjs'] });
  assert.equal(result.status, 'flagged');
  assert.deepEqual(result.findings.map(({ file, name, unused }) => [file, name, unused]),
    [['src/paths.mjs', 'pathsEqual', true], ['src/main.mjs', 'run', true]]);
  assert.match(result.findings[0].reason, /^Shadow module: src\/paths\.mjs exports pathsEqual, but no product module uses it \(only tests, or nothing\); wire it into the production caller the task names, make it module-private, or delete it\./);
});

test('test-only diffs and non-git worktrees run no comparison', async (context) => {
  const worktree = repo(context);
  assert.deepEqual(await checkShadowModules({ worktree, files: ['tests/store.test.mjs', 'README.md'] }), { status: 'none', findings: [] });
  const plain = mkdtempSync(path.join(tmpdir(), 'roster-shadow-plain-'));
  context.after(() => rmSync(plain, { recursive: true, force: true }));
  assert.equal((await checkShadowModules({ worktree: plain, files: ['src/x.mjs'] })).status, 'unavailable');
});

test('the coder gets one shadow-module correction and RESULT.md shows the gate to the reviewer', async (context) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-shadow-root-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'), readFileSync(new URL('../principals/coder.md', import.meta.url), 'utf8'));
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  const worktree = repo(context);
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nCode carefully.\n');
  writeFileSync(path.join(worktree, 'TASK.md'), '---\nreference: issue:7\ntask_class: feat\ndifficulty: 3\n---\n' +
    '# Task\n\n## Ask\nAdd `readRecords`.\n\n## Acceptance checks\n1. `readRecords()` returns appended records.\n\n' +
    '## Files allowed\n- src/record-store.mjs\n- src/records.mjs\n- tests/store.test.mjs\n');
  git(worktree, 'add', '-A');
  git(worktree, 'commit', '-q', '-m', 'task');
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
    .replace('base_url: ""', 'base_url: http://localhost:3456/v1').replace('model: ""', 'model: local-model')
    .replace('turn_budget: 1000', 'turn_budget: 4'));
  const call = (id, name, args) => ({ id, type: 'function', function: { name, arguments: JSON.stringify(args) } });
  const reply = (message) => ({ status: 200, json: async () => ({ choices: [{
    finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message: { role: 'assistant', content: null, ...message } }],
  usage: { prompt_tokens: 5, completion_tokens: 2 } }) });
  const recordsTest = (from) => baseTest + `import { readRecords } from '../src/${from}';\n` +
    "test('reads records', () => assert.equal(readRecords().length, readAll().length));\n";
  const calls = [];
  const fetchImpl = async (_url, request) => {
    const body = JSON.parse(request.body);
    if (body.messages[0].content.startsWith('You are the coder seat reading your own diff')) {
      return reply({ content: JSON.stringify({ checks: [{ id: 1, met: true, evidence: 'readRecords in record-store' }], findings: [] }) });
    }
    calls.push(body);
    if (calls.length === 1) return reply({ tool_calls: [
      call('r1', 'read_file', { path: 'src/record-store.mjs' }), call('r2', 'read_file', { path: 'tests/store.test.mjs' }),
      call('w1', 'write_file', { path: 'src/records.mjs', content: "import { readAll } from './record-store.mjs';\nexport const readRecords = () => readAll();\n" }),
      call('w2', 'write_file', { path: 'tests/store.test.mjs', content: recordsTest('records.mjs') })] });
    if (calls.length === 3) return reply({ tool_calls: [
      call('w3', 'write_file', { path: 'src/record-store.mjs', content: `${store}export const readRecords = () => readAll();\n` }),
      call('d1', 'delete_file', { path: 'src/records.mjs' }),
      call('w4', 'write_file', { path: 'tests/store.test.mjs', content: recordsTest('record-store.mjs') })] });
    return reply({ content: 'Added readRecords.' });
  };
  const events = [];
  const result = await runCoder({ repoRoot, worktree, config, task: 'issue-7', session: 'roster-session', env: {},
    onEvent: async (event) => { if (event.type === 'shadow-modules') events.push(event); },
    fetchImpl: withResearchSummary(fetchImpl) });
  assert.equal(calls.length, 4);
  const correction = calls[2].messages.at(-1).content;
  assert.match(correction, /^Shadow module: new file src\/records\.mjs adds readRecords/);
  assert.match(correction, /One shadow-module correction is allowed/);
  assert.deepEqual(events.map(({ status, findings }) => [status, findings]), [['flagged', 1], ['checked', 0]]);
  assert.equal(existsSync(path.join(worktree, 'src', 'records.mjs')), false);
  assert.equal(result.excellence.pass, true);
  assert.match(readFileSync(result.resultPath, 'utf8'), /## Shadow modules\n\nStatus: checked\n\n- No new export duplicates an existing module or lacks a product caller\./);
});

test('shadow-module results reach the run log', async (context) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-shadow-log-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  let text = '';
  const options = { repoRoot, session: 'roster-7-coder', env: {}, errorOutput: { write(value) { text += value; } } };
  const logger = await createRunLog(options);
  await logger.seat('coder', options.session, { llm: { base_url: '', model: '' } }, async (onEvent) => {
    await onEvent({ type: 'shadow-modules', status: 'flagged', findings: 2 });
    assert.match((await readLastRunLog(options)).lastLine, /seat coder shadow-modules flagged findings=2$/);
  });
  assert.match(text, /2 new exports duplicate existing modules; one repair before review\./);
});
