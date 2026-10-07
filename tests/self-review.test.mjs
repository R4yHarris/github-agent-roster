import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createRunLog, readLastRunLog } from '../src/lib/run-log.mjs';
import { needsSelfReview, parseSelfReview, runSelfReview, selfReviewReasons } from '../src/runtime/self-review.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { withResearchSummary } from './helpers/research.mjs';

const git = (cwd, ...args) => execFileSync('git', ['-c', 'core.autocrlf=false', '-c', 'user.email=t@example.com', '-c', 'user.name=t', ...args],
  { cwd, encoding: 'utf8' });
const baseTest = "import assert from 'node:assert/strict';\nimport test from 'node:test';\n" +
  "import { add } from '../src/math.mjs';\n\ntest('adds', () => assert.equal(add(1, 2), 3));\n";
const subTest = baseTest.replace('import { add }', 'import { add, sub }') + "test('subtracts', () => assert.equal(sub(3, 1), 2));\n";
const clean = 'export const add = (a, b) => a + b;\nexport const sub = (a, b) => a - b;\n';
const debug = 'export const add = (a, b) => a + b;\nexport const sub = (a, b) => {\n  console.log(a, b);\n  return a - b;\n};\n';
const isSelfReview = (body) => body.messages[0].content.startsWith('You are the coder seat reading your own diff');
const reply = (message) => ({ status: 200, json: async () => ({ choices: [{
  finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message: { role: 'assistant', content: null, ...message } }],
usage: { prompt_tokens: 5, completion_tokens: 2, total_tokens: 7 } }) });
const report = (findings, met = true) => reply({ content: JSON.stringify({
  checks: [{ id: 1, met, evidence: 'src/math.mjs exports sub' }], findings }) });
const write = (id, file, content) => ({ id, type: 'function', function: { name: 'write_file',
  arguments: JSON.stringify({ path: file, content }) } });
const read = (id, file) => ({ id, type: 'function', function: { name: 'read_file', arguments: JSON.stringify({ path: file }) } });

function fixture(context) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-self-review-root-'));
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-self-review-'));
  context.after(() => {
    rmSync(repoRoot, { recursive: true, force: true });
    rmSync(worktree, { recursive: true, force: true });
  });
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'), readFileSync(new URL('../principals/coder.md', import.meta.url), 'utf8'));
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  mkdirSync(path.join(worktree, 'src'));
  mkdirSync(path.join(worktree, 'tests'));
  writeFileSync(path.join(worktree, 'package.json'), '{ "type": "module" }\n');
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nCode carefully.\n');
  writeFileSync(path.join(worktree, 'src', 'math.mjs'), 'export const add = (a, b) => a + b;\n');
  writeFileSync(path.join(worktree, 'tests', 'math.test.mjs'), baseTest);
  writeFileSync(path.join(worktree, 'TASK.md'), '---\nreference: issue:6\ntask_class: feat\ndifficulty: 3\n---\n' +
    '# Task\n\n## Ask\nAdd `sub` to `src/math.mjs`.\n\n## Acceptance checks\n1. `sub(3, 1)` returns 2.\n\n' +
    '## Design\n\nExtend src/math.mjs with sub.\n\n## Files allowed\n- src/math.mjs\n- tests/math.test.mjs\n');
  git(worktree, 'init', '-q');
  git(worktree, 'add', '-A');
  git(worktree, 'commit', '-q', '-m', 'base');
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
    .replace('base_url: ""', 'base_url: http://localhost:3456/v1').replace('model: ""', 'model: local-model')
    .replace('turn_budget: 1000', 'turn_budget: 4'));
  return { repoRoot, worktree, config };
}

async function drive(context, selfReviewReply) {
  const options = fixture(context);
  const coderCalls = [];
  const reviews = [];
  const events = [];
  const fetchImpl = async (_url, request) => {
    const body = JSON.parse(request.body);
    if (isSelfReview(body)) {
      reviews.push(body);
      return selfReviewReply(reviews.length);
    }
    coderCalls.push(body);
    if (coderCalls.length === 1) return reply({ tool_calls: [read('r1', 'src/math.mjs'), read('r2', 'tests/math.test.mjs'),
      write('s1', 'src/math.mjs', debug), write('t1', 'tests/math.test.mjs', subTest)] });
    if (coderCalls.length === 3) return reply({ tool_calls: [write('s2', 'src/math.mjs', clean)] });
    return reply({ content: 'Added sub with a red/green test.' });
  };
  const result = await runCoder({ ...options, task: 'issue-6', session: 'roster-session', env: {},
    onEvent: async (event) => { if (event.type === 'self-review') events.push(event); },
    fetchImpl: withResearchSummary(fetchImpl) });
  return { ...options, result, coderCalls, reviews, events };
}

test('a self-review finding gets one repair before review and is kept in RESULT.md and the coder ledger', async (context) => {
  const run = await drive(context, () => report(['src/math.mjs sub leaves a console.log debug line']));
  assert.equal(run.reviews.length, 1, 'self-review runs once; the independent reviewer judges the repair');
  assert.equal(run.coderCalls.length, 4);
  const correction = run.coderCalls[2].messages.at(-1).content;
  assert.match(correction, /^Self-review: src\/math\.mjs sub leaves a console\.log debug line/);
  assert.match(correction, /One self-review correction is allowed/);
  assert.equal(readFileSync(path.join(run.worktree, 'src', 'math.mjs'), 'utf8'), clean);
  assert.equal(run.result.excellence.pass, true);
  assert.equal(run.result.selfReview.status, 'findings');
  assert.deepEqual(run.events.map(({ status, findings, input, output }) => [status, findings, input, output]), [['findings', 1, 5, 2]]);
  assert.equal(run.result.usage.prompt_tokens, 5 * (run.coderCalls.length + run.reviews.length),
    'self-review cost counts toward the coder run');
  assert.match(readFileSync(run.result.resultPath, 'utf8'),
    /## Self-review\n\nStatus: findings \(\d+ ms\)\n\n- src\/math\.mjs sub leaves a console\.log debug line/);
  const memory = readFileSync(path.join(run.repoRoot, run.config.paths.memory), 'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(memory.at(-1).self_review, 'src/math.mjs sub leaves a console.log debug line');
});

test('a clean self-review goes straight on to the reviewer with no correction', async (context) => {
  const run = await drive(context, () => report([]));
  assert.equal(run.reviews.length, 1);
  assert.equal(run.coderCalls.length, 2);
  assert.equal(run.result.selfReview.status, 'clean');
  assert.equal(run.result.excellence.pass, true);
  const [body] = run.reviews;
  assert.equal(body.tools, undefined, 'self-review has no tools');
  assert.match(body.messages[1].content, /## Acceptance checks\n\n1\. `sub\(3, 1\)` returns 2\.[\s\S]*## Design\n\nExtend src\/math\.mjs[\s\S]*## Red\/green[\s\S]*## Diff/);
});

test('self-review cannot write files or publish: a tool request fails open without touching the worktree', async (context) => {
  const options = fixture(context);
  writeFileSync(path.join(options.worktree, 'src', 'math.mjs'), clean);
  const before = git(options.worktree, 'status', '--porcelain');
  const result = await runSelfReview({ ...options, task: readFileSync(path.join(options.worktree, 'TASK.md'), 'utf8'),
    files: ['src/math.mjs'], env: {}, fetchImpl: async () => reply({ tool_calls: [
      write('w', 'src/math.mjs', 'export {};\n'),
      { id: 'p', type: 'function', function: { name: 'run_command', arguments: '{"command":"git push"}' } }] }) });
  assert.equal(result.status, 'unavailable');
  assert.match(result.reason, /cannot request tools/);
  assert.equal(git(options.worktree, 'status', '--porcelain'), before);
  assert.equal(readFileSync(path.join(options.worktree, 'src', 'math.mjs'), 'utf8'), clean);
});

test('self-review output follows a strict schema and only product-code diffs need it', () => {
  assert.deepEqual(parseSelfReview('{"checks":[{"id":1,"met":false,"evidence":"no sub"}],"findings":[]}', 1),
    { checks: [{ id: 1, met: false, evidence: 'no sub' }], findings: [] });
  for (const bad of ['prose', '{"checks":[],"findings":[]}', '{"checks":[{"id":1,"met":true,"evidence":"x"}],"findings":[],"verdict":"pass"}',
    `{"checks":[{"id":1,"met":true,"evidence":"x"}],"findings":${JSON.stringify(Array(9).fill('f'))}}`]) {
    assert.throws(() => parseSelfReview(bad, 1));
  }
  assert.deepEqual(selfReviewReasons({ checks: [{ id: 2, met: false, evidence: 'missing' }], findings: ['debug line'] }),
    ['Self-review: check 2 unmet: missing', 'Self-review: debug line']);
  assert.equal(needsSelfReview(['src/a.mjs', 'tests/a.test.mjs']), true);
  assert.equal(needsSelfReview(['tests/a.test.mjs', 'README.md']), false);
});

test('self-review time and tokens reach the run log', async (context) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-self-review-log-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  let text = '';
  const options = { repoRoot, session: 'roster-6-coder', env: {}, errorOutput: { write(value) { text += value; } } };
  const logger = await createRunLog(options);
  await logger.seat('coder', options.session, { llm: { base_url: '', model: '' } }, async (onEvent) => {
    await onEvent({ type: 'self-review', status: 'findings', unmet: 1, findings: 2, ms: 840, input: 312, output: 40 });
    assert.match((await readLastRunLog(options)).lastLine, /seat coder self-review findings unmet=1 findings=2 ms=840 in=312 out=40$/);
  });
  assert.match(text, /Self-review found 1 unmet checks and 2 findings; one repair before review\./);
});
