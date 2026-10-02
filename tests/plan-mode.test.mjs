import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { createTools, planArtifactFiles } from '../src/runtime/tools.mjs';
import { runPlanner } from '../src/seats/planner.mjs';
import { formatHelp } from '../src/shell/commands.mjs';
import { formatTray } from '../src/shell/tray.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-plan-mode-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const worktree = path.join(root, 'worktree');
  mkdirSync(worktree);
  writeFileSync(path.join(worktree, 'README.md'), '# Original\n');
  return { root, worktree };
}

test('explicit plan mode writes only PLAN and preserves product files without TASK or recipe', async (t) => {
  const { root, worktree } = fixture(t);
  const plan = await runPlanner({ worktree, repoRoot: root, config, env: {}, ask: 'Add Status to README.md.',
    reference: 'local:test', task: 'local-test', session: 'planner-test', askKind: 'slice', planMode: true });
  assert.equal(existsSync(plan.planPath), true);
  assert.equal(existsSync(path.join(worktree, 'TASK.md')), false);
  assert.equal(existsSync(path.join(worktree, 'RECIPE.yml')), false);
  assert.equal(existsSync(path.join(worktree, 'ESTIMATE.md')), false);
  assert.equal(readFileSync(path.join(worktree, 'README.md'), 'utf8'), '# Original\n');
  assert.match(readFileSync(plan.planPath, 'utf8'), /^# Plan:/m);
});

test('plan exploration reads source but denies product, TASK, recipe and private artifact access', async (t) => {
  const { worktree } = fixture(t);
  const tools = await createTools({ worktree, seat: 'planner', plannerArtifacts: planArtifactFiles,
    plannerReads: true, env: {} });
  assert.equal(await tools.read_file({ path: 'README.md' }), '# Original\n');
  assert.equal(Object.hasOwn(tools, 'run_test'), false);
  for (const file of ['README.md', 'TASK.md', 'RECIPE.yml']) {
    await assert.rejects(tools.write_file({ path: file, content: 'forged' }), /Planner write_file allows only root PLAN/);
  }
  await assert.rejects(tools.read_file({ path: '.roster/config.yml' }), /private \.roster/);
});

test('configured planner can read then return a validated PLAN-only description', async (t) => {
  const { root, worktree } = fixture(t);
  let calls = 0;
  const configured = { ...config, llm: { ...config.llm, base_url: 'http://localhost:8000/v1', model: 'local-model' } };
  const plan = await runPlanner({ worktree, repoRoot: root, config: configured, env: {}, ask: 'Add Status to README.md.',
    reference: 'local:test', task: 'local-test', session: 'planner-test', askKind: 'slice', planMode: true,
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.deepEqual(body.tools.map(({ function: tool }) => tool.name), ['read_file', 'write_file', 'list_dir', 'search_text']);
      assert.deepEqual(body.tools.find(({ function: tool }) => tool.name === 'write_file').function.parameters.properties.path.enum, ['PLAN.md']);
      return Response.json({ choices: [{ finish_reason: calls === 1 ? 'tool_calls' : 'stop', message: calls === 1
        ? { role: 'assistant', tool_calls: [{ id: 'read', type: 'function',
          function: { name: 'read_file', arguments: '{"path":"README.md"}' } }] }
        : { role: 'assistant', content: JSON.stringify({ title: 'Add Status', acceptance_checks: ['node --test exits 0'],
          files_allowed: ['README.md'] }) } }] });
    } });
  assert.equal(calls, 2);
  assert.ok(plan.planPath);
  assert.equal(existsSync(path.join(worktree, 'TASK.md')), false);
});

test('plan Enter accepts once and stop preserves the existing plan without invoking coder', async () => {
  const calls = [];
  const paused = { local: true, task: 'local-test', worktreePath: 'local-test', planMode: true,
    planPath: 'PLAN.md', askKind: 'slice', planningOnly: true };
  const shell = createDispatcher({ config, env: {}, output: { write() {} }, errorOutput: { write() {} },
    services: { repositoryBranch: () => 'main', runBuiltinAsk: async (_ask, options) => {
      calls.push(options);
      return options.planMode ? paused : { ...paused, planMode: false, planPath: undefined,
        planningOnly: false, review: { verdict: 'pass' } };
    } } });
  await shell.dispatch('/plan Add Status to README.md.');
  assert.equal(shell.state.display.state, 'planning');
  assert.match(formatTray(shell.state.display, { color: false }).bottom, /plan/);
  await shell.dispatch('');
  assert.equal(calls[1].acceptPlan, true);
  assert.equal(calls[1].preparedRun, paused);
  assert.equal(calls[1].planMode, false);
  await shell.dispatch('/plan Add Status to README.md.');
  await shell.dispatch('/stop');
  assert.equal(shell.state.pendingConfirm, null);
  assert.equal(calls.length, 3);
  assert.match(formatHelp('plan'), /--plan/);
});
