import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { withResearchSummary } from './helpers/research.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const base = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 1000', 'turn_budget: 3'));
const config = { ...base, tools: { ...base.tools, internet: true } };

function fixture(context, ask) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-toolset-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'),
    readFileSync(new URL('../principals/coder.md', import.meta.url), 'utf8'));
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nCode carefully.\n');
  writeFileSync(path.join(worktree, 'TASK.md'),
    planStub(ask, { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task);
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  writeFileSync(path.join(worktree, 'smoke.test.mjs'), '// Test fixture scope.\n');
  return { repoRoot, worktree, config, task: 'issue-4', session: 'roster-session',
    memoryPath: path.join(repoRoot, config.paths.memory) };
}

async function offered(context, ask) {
  const options = fixture(context, ask);
  let tools;
  const events = [];
  await runCoder({
    ...options, env: {}, onEvent: (event) => events.push(event),
    fetchImpl: withResearchSummary(async (_url, request) => {
      tools ??= JSON.parse(request.body).tools.map(({ function: tool }) => tool.name);
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Inspected README.' } }] }) };
    }),
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  }).catch(() => {});
  return { tools, logged: events.find(({ type }) => type === 'toolset')?.tools };
}

test('a default code slice is offered no web tools and no run_command, even with internet enabled', async (context) => {
  const { tools, logged } = await offered(context, 'Update `README.md` with a Status section; keep `smoke.test.mjs` in scope.');
  for (const name of ['web_search', 'web_fetch', 'run_command']) assert.ok(!tools.includes(name), name);
  assert.deepEqual(logged, tools);
});

test('a task that names git diff opts in to run_command; web research opts in to web_search only', async (context) => {
  const command = await offered(context,
    'Update `README.md` after checking `git diff`; keep `smoke.test.mjs` in scope.');
  assert.ok(command.tools.includes('run_command'));
  assert.ok(!command.tools.includes('web_search'));
  assert.deepEqual(command.logged, command.tools);
  const web = await offered(context,
    'Update `README.md` after checking `git diff`; cite one source found with web_search; keep `smoke.test.mjs` in scope.');
  assert.ok(web.tools.includes('web_search'));
  assert.ok(!web.tools.includes('web_fetch'));
  assert.ok(!web.tools.includes('run_command'), 'untrusted web pages never share a turn with run_command');
});
