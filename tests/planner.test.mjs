import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { parseRecipe } from '../src/lib/recipe.mjs';
import { planAsk, planStub, renderAsk, renderAssignment, taskFilesAllowed } from '../src/planner/stub.mjs';
import { runPlanner } from '../src/seats/planner.mjs';

const configExample = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const stubConfig = parseConfig(configExample);
const llmConfig = parseConfig(configExample.replace('base_url: ""', 'base_url: "http://localhost:1234/v1"')
  .replace('model: ""', 'model: test-model'));

test('stub creates an executable planner/coder/reviewer recipe and task without an LLM request', async () => {
  const ask = 'Add a Status section to `README.md`.\n\nKeep the text short.';
  const plan = await planAsk(ask, { config: stubConfig, reference: 'issue:42', title: 'Update project status',
    fetchImpl: () => { throw new Error('stub must not make a network request'); } });
  assert.deepEqual(parseRecipe(plan.recipe), {
    version: 1,
    ask: 'issue:42',
    seats: [
      { id: 'planner', principal: 'coder', worker: 'builtin',
        sequence: ['read_ask', 'plan', 'write_task'] },
      { id: 'coder', principal: 'coder', worker: 'builtin',
        sequence: ['load_context', 'implement', 'run_tests', 'summarize'] },
      { id: 'reviewer', principal: 'reviewer', worker: 'builtin',
        sequence: ['read_diff', 'check_acceptance', 'write_review'] },
    ],
  });
  assert.match(plan.task, /^# Task: Update project status/m);
  assert.match(plan.task, /- node --test exits 0/);
  assert.match(plan.task, /## Ask\nAdd a Status section to `README\.md`/);
  assert.deepEqual(taskFilesAllowed(plan.task), ['README.md']);
  assert.equal(plan.usage, null);
  assert.equal(plan.turns, 0);
  assert.equal(renderAsk(ask), `# Ask\n\n${ask}\n`);
  assert.equal(renderAssignment({
    number: 42, url: 'https://github.com/example/roster/issues/42',
    title: 'Example task', body: ask,
  }), `# Assignment\n\n- Issue URL: https://github.com/example/roster/issues/42\n` +
    `- Issue number: 42\n- Title: Example task\n\n## Ask\n\n${ask}\n`);
});

test('stub uses explicit acceptance checks and allowed paths, or a broad local draft when none are known', () => {
  const plan = planStub('Implement this.\n\n## Acceptance checks\n- `node --test` exits 0\n' +
    '- README has a Status section\n\n## Files allowed\n- `README.md`\n- `src/**`\n');
  assert.equal(parseRecipe(plan.recipe).ask, 'local:draft');
  assert.match(plan.task, /- README has a Status section/);
  assert.deepEqual(taskFilesAllowed(plan.task), ['README.md', 'src/**']);
  assert.deepEqual(taskFilesAllowed(planStub('Do the thing.').task), ['**/*']);
  assert.throws(() => planStub('Write secrets.\n## Files allowed\n- `.env`'), /protected files/);
  assert.throws(() => planStub(Array.from({ length: 33 }, (_, index) =>
    `Update src/file${index}.mjs`).join('\n')), /Files allowed must contain 1-32 entries/);
  assert.throws(() => planStub(' '), /Ask must be nonempty/);
});

test('board metadata seeds the stub task and supplies defaults to an LLM plan', async () => {
  const metadata = { task_class: 'fix', difficulty: 4, estimate_min: 35 };
  const stub = planStub('Fix `README.md`.', { reference: 'issue:42', title: 'Fix README', metadata });
  assert.match(stub.task, /difficulty: 4\nestimate_min: 35\ntask_class: fix\n/);
  const planned = await planAsk('Fix `README.md`.', {
    config: llmConfig, reference: 'issue:42', title: 'Fix README', metadata,
    fetchImpl: async () => ({ status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
        title: 'Fix README', acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'],
      }) } }],
    }) }),
    env: {},
  });
  assert.match(planned.task, /difficulty: 4\nestimate_min: 35\ntask_class: fix\n/);
});

test('a routed planner cannot silently switch to a model outside the selected fleet profile', async () => {
  const config = { ...llmConfig, planner: { turn_budget: 1 } };
  await assert.rejects(planAsk('Update README.md.', {
    config, env: {}, lockedModel: 'selected-model',
    fetchImpl: async () => ({ status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
        title: 'Update README', acceptance_checks: ['node --test exits 0'],
        files_allowed: ['README.md'], model: 'unregistered-model',
      }) } }],
    }) }),
  }), /routed planner must keep the selected fleet model/);
  const plan = await planAsk('Update README.md.', {
    config, env: {}, lockedModel: 'selected-model',
    fetchImpl: async () => ({ status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
        title: 'Update README', acceptance_checks: ['node --test exits 0'],
        files_allowed: ['README.md'], model: '',
      }) } }],
    }) }),
  });
  assert.match(plan.task, /model: selected-model/);
});
test('planner accepts CRLF Ask templates while rejecting lone control characters', () => {
  const plan = planStub('Update README.md.\r\n\r\n## Acceptance checks\r\n- node --test exits 0\r\n');
  assert.match(plan.task, /## Ask\nUpdate README\.md\.\n\n## Acceptance checks/);
  assert.doesNotMatch(plan.task, /\r/);
  assert.throws(() => planStub('Invalid\rlone return'), /Ask must be nonempty UTF-8/);
});

test('LLM planner validates JSON before generating a recipe and never exposes the key in failures', async () => {
  let calls = 0;
  const fetchImpl = async (url, options) => {
    calls += 1;
    assert.equal(String(url), 'http://localhost:1234/v1/chat/completions');
    assert.equal(options.method, 'POST');
    assert.equal(options.headers.Authorization, 'Bearer secret-value');
    const body = JSON.parse(options.body);
    assert.equal(body.model, 'test-model');
    assert.equal(body.messages[1].content, 'Add status to README.');
    assert.equal(body.tools, undefined);
    return {
      status: 200,
      async json() {
        return { choices: [{ message: { role: 'assistant', content: JSON.stringify({
          title: 'Add a Status section',
          acceptance_checks: ['README has a Status section', 'node --test exits 0'],
          files_allowed: ['README.md'],
        }) } }], usage: { prompt_tokens: 30, completion_tokens: 11 } };
      },
    };
  };
  const plan = await planAsk('Add status to README.', {
    config: llmConfig, reference: 'issue:8', fetchImpl, env: { ROSTER_API_KEY: 'secret-value' },
  });
  assert.equal(calls, 1);
  assert.equal(parseRecipe(plan.recipe).ask, 'issue:8');
  assert.deepEqual(taskFilesAllowed(plan.task), ['README.md']);
  assert.deepEqual(plan.usage, { prompt_tokens: 30, completion_tokens: 11 });
  assert.equal(plan.turns, 1);
  await assert.rejects(planAsk('Add status to README.', {
    config: llmConfig, fetchImpl: async () => ({ ok: false, status: 401 }),
    env: { ROSTER_API_KEY: 'secret-value' },
  }), (error) => {
    assert.match(error.message, /HTTP 401/);
    assert.ok(!error.message.includes('secret-value'));
    return true;
  });
  await assert.rejects(planAsk('Add status to README.', {
    config: llmConfig, fetchImpl: async () => ({ status: 200, json: async () => ({
      choices: [{ message: { role: 'assistant', content: JSON.stringify({
        title: 'Bad plan', acceptance_checks: ['test'], files_allowed: ['../secrets'],
      }) } }],
    }) }),
    env: { ROSTER_API_KEY: 'test-key' },
  }), /protected files/);
});

test('planner repairs invalid JSON within its configured budget and fails when exhausted', async () => {
  let calls = 0;
  const plan = await planAsk('Update README.md.', {
    config: llmConfig, env: {}, vault: { get: async () => undefined },
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.tools, undefined);
      if (calls === 2) assert.match(body.messages.at(-1).content, /invalid JSON/);
      return { status: 200, json: async () => ({
        choices: [{ message: { role: 'assistant', content: calls === 1 ? '{' : JSON.stringify({
          title: 'Update README', acceptance_checks: ['node --test exits 0'],
          files_allowed: ['README.md'],
        }) } }],
        usage: { prompt_tokens: calls, completion_tokens: 2 },
      }) };
    },
  });
  assert.equal(calls, 2);
  assert.equal(plan.turns, 2);
  assert.deepEqual(plan.usage, { prompt_tokens: 3, completion_tokens: 4 });

  const singleTurn = parseConfig(configExample.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
    .replace('model: ""', 'model: test-model').replace('turn_budget: 2', 'turn_budget: 1'));
  let failures = 0;
  await assert.rejects(planAsk('Update README.md.', {
    config: singleTurn, env: {}, vault: { get: async () => undefined },
    fetchImpl: async () => {
      failures += 1;
      return { status: 200, json: async () => ({ choices: [{ message: { role: 'assistant', content: '{' } }] }) };
    },
  }), /Planner turn budget \(1\) exhausted: LLM planner returned invalid JSON/);
  assert.equal(failures, 1);
});

test('planner rejects write_file requests without touching source or task files', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  await assert.rejects(runPlanner({
    worktree, repoRoot,
    issue: { number: 42, title: 'Protect the task', body: 'Edit `src/app.mjs`.' },
    config: llmConfig, env: {}, vault: { get: async () => undefined },
    fetchImpl: async (_url, request) => {
      assert.equal(JSON.parse(request.body).tools, undefined);
      return { status: 200, json: async () => ({ choices: [{ finish_reason: 'tool_calls',
        message: { role: 'assistant', tool_calls: [{ id: 'write', type: 'function', function: {
          name: 'write_file',
          arguments: JSON.stringify({ path: 'src/app.mjs', content: 'bad' }),
        } }] },
      }] }) };
    },
  }), /planner cannot call tools/i);
  assert.equal(existsSync(join(worktree, 'src', 'app.mjs')), false);
  assert.equal(existsSync(join(worktree, 'RECIPE.yml')), false);
  assert.equal(existsSync(join(worktree, 'TASK.md')), false);
  assert.equal(JSON.parse(readFileSync(join(repoRoot, '.roster', 'memory', 'planner.jsonl'), 'utf8')).status,
    'failed');
});
