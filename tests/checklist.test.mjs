import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createRunLog, readLastRunLog } from '../src/lib/run-log.mjs';
import { planStub } from '../src/planner/stub.mjs';
import {
  checklistProgress, checklistTable, closeFromSummary, createChecklist, openItems, updateChecklist,
} from '../src/runtime/checklist.mjs';
import { writeResult } from '../src/runtime/excellence.mjs';
import { runCoder as runCoderSeat } from '../src/seats/coder.mjs';
import { withResearchSummary } from './helpers/research.mjs';
import { formatTray } from '../src/shell/tray.mjs';

const runCoder = (options) => runCoderSeat({ ...options, fetchImpl: withResearchSummary(options.fetchImpl) });

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 1000', 'turn_budget: 3'));

function worktreeFixture(context) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-checklist-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true }));
  return { worktree };
}

function coderFixture(context) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-checklist-coder-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'),
    readFileSync(new URL('../principals/coder.md', import.meta.url), 'utf8'));
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nCode carefully.\n');
  writeFileSync(path.join(worktree, 'TASK.md'),
    planStub('Update `README.md` with a Status section and keep `smoke.test.mjs` in scope.',
      { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task);
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  writeFileSync(path.join(worktree, 'smoke.test.mjs'), '// Test fixture scope.\n');
  return { repoRoot, worktree, config, task: 'issue-4', session: 'roster-session',
    memoryPath: path.join(repoRoot, config.paths.memory) };
}

const reply = (message, finish = message.tool_calls ? 'tool_calls' : 'stop') => ({ status: 200, json: async () => ({
  choices: [{ finish_reason: finish, message: { role: 'assistant', content: null, ...message } }],
  usage: { prompt_tokens: 5, completion_tokens: 2 },
}) });
const call = (id, name, args) => ({ id, type: 'function', function: { name, arguments: JSON.stringify(args) } });

test('update_checklist validates ids, statuses, evidence, and one in_progress item', () => {
  const checklist = createChecklist(['first', 'second']);
  assert.throws(() => updateChecklist(checklist, { items: [{ id: 3, status: 'done', evidence: 'x' }] }), TypeError);
  assert.throws(() => updateChecklist(checklist, { items: [{ id: 1, status: 'finished' }] }), TypeError);
  assert.throws(() => updateChecklist(checklist, { items: [{ id: 1, status: 'done' }] }), /needs evidence/);
  assert.throws(() => updateChecklist(checklist, {
    items: [{ id: 1, status: 'in_progress' }, { id: 2, status: 'in_progress' }] }), /one item at a time/);
  assert.equal(checklist.items.every(({ status }) => status === 'pending'), true);
  assert.match(updateChecklist(checklist, { items: [{ id: 1, status: 'done', evidence: 'src/a.mjs a' }] }),
    /1\. \[done\] first \(evidence: src\/a\.mjs a\)/);
  assert.deepEqual(checklistProgress(checklist), { done: 1, total: 2 });
  assert.deepEqual(openItems(checklist).map(({ id }) => id), [2]);
});

test('a final summary closes the checks it reports and leaves the others open', () => {
  const checklist = createChecklist(['first', 'second', 'third']);
  closeFromSummary(checklist, 'Summary\n1. done: src/a.mjs exports a\nCheck 2 - blocked: needs a human key\nNothing else.');
  assert.deepEqual(checklist.items.map(({ status }) => status), ['done', 'blocked', 'pending']);
  assert.equal(checklist.items[0].evidence, 'src/a.mjs exports a');
  assert.match(checklistTable(checklist), /\| 2 \| second \| blocked \| needs a human key \|/);
});

test('an engaged coder that finishes with an open item is corrected, then finishes once all items are closed', async (context) => {
  const options = coderFixture(context);
  const events = [];
  let calls = 0;
  const fetchImpl = async (_url, request) => {
    calls += 1;
    const body = JSON.parse(request.body);
    assert.ok(body.tools.some(({ function: tool }) => tool.name === 'update_checklist'));
    if (calls === 1) {
      return reply({ tool_calls: [
        call('w1', 'write_file', { path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
        call('c1', 'update_checklist', { items: [{ id: 1, status: 'in_progress' }] }),
        call('t1', 'run_test', {}),
      ] });
    }
    if (calls === 2) return reply({ content: 'Done.' });
    if (calls === 3) {
      const correction = body.messages.at(-1).content;
      assert.match(correction, /Your checklist still has open items/);
      const open = [...correction.matchAll(/^(\d+)\. \[/gm)].map(([, id]) => Number(id));
      assert.ok(open.length >= 1);
      return reply({ tool_calls: [call('c2', 'update_checklist', {
        items: open.map((id) => ({ id, status: 'done', evidence: `README.md Status, check ${id}` })),
      })] });
    }
    return reply({ content: 'All checks closed.' });
  };
  const result = await runCoder({ ...options, env: {}, fetchImpl,
    runTestCommand: async () => ({ stdout: 'all tests pass', stderr: '' }),
    onEvent: async (event) => { if (event.type === 'checklist') events.push(event); } });
  assert.equal(calls, 4);
  assert.equal(result.summary, 'All checks closed.');
  assert.equal(result.checklist.every(({ status }) => status === 'done'), true);
  assert.equal(result.checklistCorrections, 1);
  assert.ok(events.some(({ open }) => open === true));
  assert.deepEqual(events.at(-1), { type: 'checklist', done: result.checklist.length, total: result.checklist.length });
  assert.match(readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8'), /## Checklist\n/);
});

test('a coder that never uses the checklist is not corrected; its summary closes the checks it reports', async (context) => {
  const options = coderFixture(context);
  let calls = 0;
  const result = await runCoder({ ...options, env: {},
    fetchImpl: async () => {
      calls += 1;
      return calls === 1
        ? reply({ tool_calls: [call('w1', 'write_file', { path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
          call('t1', 'run_test', {})] })
        : reply({ content: '1. done: README.md has a Status section' });
    },
    runTestCommand: async () => ({ stdout: 'all tests pass', stderr: '' }) });
  assert.equal(calls, 2);
  assert.equal(result.checklistCorrections, undefined);
  assert.equal(result.checklist[0].status, 'done');
  assert.equal(result.checklist[0].evidence, 'README.md has a Status section');
});

test('RESULT.md carries a per-check evidence table', async (context) => {
  const { worktree } = worktreeFixture(context);
  const checklist = createChecklist(['Exports two | not one']);
  updateChecklist(checklist, { items: [{ id: 1, status: 'done', evidence: 'src/a.mjs a' }] });
  await writeResult({ worktree, env: {}, apiKeyEnv: 'ROSTER_API_KEY',
    result: { summary: 'ok', checklist: checklist.items, tests: { exit_code: 0, stdout: 'ok', stderr: '' } },
    excellence: { pass: true, reasons: [], files: [] } });
  const text = readFileSync(path.join(worktree, 'RESULT.md'), 'utf8');
  assert.match(text, /## Checklist\n\n\| # \| Check \| Status \| Evidence \|/);
  assert.match(text, /\| 1 \| Exports two \\\| not one \| done \| src\/a\.mjs a \|/);
});

test('checklist progress reaches the run log and the status rail', async (context) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-checklist-log-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  let text = '';
  const options = { repoRoot, session: 'roster-9-coder', env: {}, errorOutput: { write(value) { text += value; } } };
  const logger = await createRunLog(options);
  await logger.seat('coder', options.session, { llm: { base_url: '', model: '' } }, async (onEvent) => {
    await onEvent({ type: 'checklist', done: 1, total: 3, open: true });
    assert.match((await readLastRunLog(options)).lastLine, /seat coder checklist 1\/3 open-at-finish$/);
  });
  assert.match(text, /Coder tried to finish with 2 open checklist items\./);
  assert.match(formatTray({ state: 'coding', issue: 9, checklist: '1/3' }, { color: false, columns: 200 }).rail, /#9 coding ✓1\/3 /);
  assert.doesNotMatch(formatTray({ state: 'coding', issue: 9, checklist: 'x' }, { color: false, columns: 200 }).rail, /✓/);
});
