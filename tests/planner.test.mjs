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

test('stub uses explicit acceptance checks and paths, and never invents broad scope', () => {
  const plan = planStub('Implement this.\n\n## Acceptance checks\n- `node --test` exits 0\n' +
    '- README has a Status section\n\n## Files allowed\n- `README.md`\n- `src/**`\n');
  assert.equal(parseRecipe(plan.recipe).ask, 'local:draft');
  assert.match(plan.task, /- README has a Status section/);
  assert.deepEqual(taskFilesAllowed(plan.task), ['README.md', 'src/**']);
  assert.throws(() => planStub('Do the thing.'), /must name or declare allowed files/);
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
    assert.equal(body.messages[1].content, 'Add status to README.md.');
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
  const plan = await planAsk('Add status to README.md.', {
    config: llmConfig, reference: 'issue:8', fetchImpl, env: { ROSTER_API_KEY: 'secret-value' },
  });
  assert.equal(calls, 1);
  assert.equal(parseRecipe(plan.recipe).ask, 'issue:8');
  assert.deepEqual(taskFilesAllowed(plan.task), ['README.md']);
  assert.deepEqual(plan.usage, { prompt_tokens: 30, completion_tokens: 11 });
  assert.equal(plan.turns, 1);
  await assert.rejects(planAsk('Add status to README.md.', {
    config: llmConfig, fetchImpl: async () => ({ ok: false, status: 401 }),
    env: { ROSTER_API_KEY: 'secret-value' },
  }), (error) => {
    assert.match(error.message, /HTTP 401/);
    assert.ok(!error.message.includes('secret-value'));
    return true;
  });
  await assert.rejects(planAsk('Add status to README.md.', {
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
  assert.deepEqual(plan.response, { model: llmConfig.llm.model,
    usage: { prompt_tokens: 2, completion_tokens: 2 } });

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

test('planner write_file creates TASK.md and finalizes all three artifacts before the coder', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-artifacts-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  const issue = { number: 42, title: 'Add Status', body: 'Add a one-line Status section to `README.md`.' };
  const draft = planStub(issue.body, { reference: 'issue:42', title: issue.title });
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot, issue, config: llmConfig, env: {}, vault: { get: async () => undefined },
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.deepEqual(body.tools.map(({ function: tool }) => tool.name), ['write_file']);
      assert.deepEqual(body.tools[0].function.parameters.properties.path.enum, ['RECIPE.yml', 'TASK.md', 'ESTIMATE.md']);
      if (calls === 1) return Response.json({
        choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: Object.entries({
            'RECIPE.yml': draft.recipe, 'TASK.md': draft.task, 'ESTIMATE.md': '# Draft estimate\n',
          }).map(([path, content], index) => ({ id: `plan-${index}`, type: 'function',
            function: { name: 'write_file', arguments: JSON.stringify({ path, content }) } })),
        } }],
        model: 'actual-planner-model', usage: { prompt_tokens: 100, completion_tokens: 40 },
      });
      assert.equal(readFileSync(join(worktree, 'TASK.md'), 'utf8'), draft.task);
      assert.equal(readFileSync(join(worktree, 'ESTIMATE.md'), 'utf8'), '# Draft estimate\n');
      assert.deepEqual(body.messages.slice(-3).map(({ role }) => role), ['tool', 'tool', 'tool']);
      assert.equal(JSON.parse(body.messages.at(-2).content).path, 'TASK.md');
      return Response.json({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Planning artifacts written.' } }],
        model: 'actual-planner-model', usage: { prompt_tokens: 100, completion_tokens: 40 },
      });
    },
  });
  assert.equal(calls, 1);
  assert.equal(result.turns, 1);
  assert.deepEqual(result.usage, { prompt_tokens: 100, completion_tokens: 40 });
  assert.equal(result.run.metrics.model, 'actual-planner-model');
  assert.equal(result.run.metrics.prompt_tokens, 100);
  assert.equal(result.run.metrics.completion_tokens, 40);
  assert.equal(readFileSync(result.recipePath, 'utf8'), result.recipe);
  assert.equal(readFileSync(result.taskPath, 'utf8'), result.task);
  assert.equal(readFileSync(result.estimatePath, 'utf8'), result.estimate);
  assert.match(result.estimate, /Source: task\/default/);
  assert.deepEqual(taskFilesAllowed(result.task), ['README.md']);
  assert.equal(parseRecipe(result.recipe).seats.length, 3);
  assert.equal(existsSync(join(worktree, 'README.md')), false);
});

test('planner can finalize validated JSON after writing a planning draft', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-json-tools-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot, issue: { number: 42, title: 'Update README', body: 'Update `README.md`.' },
    config: llmConfig, env: {}, fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [calls === 1 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'draft', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: '# Draft task\n' }),
        } }],
      } } : { finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
        title: 'Update README', acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'],
      }) } }] });
    },
  });
  assert.equal(calls, 2);
  assert.match(result.task, /# Task: Update README/);
  assert.equal(readFileSync(result.taskPath, 'utf8'), result.task);
});

test('planner tool errors redact known credentials before another model turn', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-safe-tool-errors-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  const secret = 'known-test-secret';
  let calls = 0;
  await runPlanner({
    worktree, repoRoot, issue: { number: 42, title: 'Update README', body: 'Update README.md.' },
    config: llmConfig, env: { ROSTER_API_KEY: secret },
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      if (calls === 1) return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'malformed', type: 'function', function: {
          name: 'write_file', arguments: secret,
        } }],
      } }] });
      assert.ok(!request.body.includes(secret));
      if (calls === 2) {
        assert.match(body.messages.at(-1).content, /Emit only tool_calls/);
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'repaired', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: '# Draft\n' }),
          } }],
        } }] });
      }
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: JSON.stringify({
          title: 'Update README', acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'],
        }),
      } }] });
    },
  });
  assert.equal(calls, 3);
});

test('a JSON tool payload embedded in planner text writes a task and finishes normally', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-sglang-text-tools-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  const ask = 'Update README.md.';
  const task = planStub(ask, { reference: 'issue:42', title: 'Update README' }).task;
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot, issue: { number: 42, title: 'Update README', body: ask }, config: llmConfig, env: {},
    fetchImpl: async (_url, request) => {
      calls += 1;
      if (calls === 1) return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: `Use this tool:\n${JSON.stringify({ name: 'write_file', arguments: { path: 'TASK.md', content: task } })}`,
        tool_calls: [],
      } }] });
      assert.equal(JSON.parse(JSON.parse(request.body).messages.at(-1).content).path, 'TASK.md');
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: 'Task ready.',
      } }] });
    },
  });
  assert.equal(calls, 1);
  assert.equal(result.error, undefined);
  assert.equal(readFileSync(result.taskPath, 'utf8'), result.task);
});

test('malformed calls use only one repair even with a one-turn configured budget and return stubs without throwing', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-sglang-failed-tools-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot, issue: { number: 42, title: 'Update README', body: 'Update README.md.' },
    config: { ...llmConfig, planner: { turn_budget: 1 } }, env: {},
    fetchImpl: async (_url, request) => {
      calls += 1;
      if (calls === 2) assert.match(JSON.parse(request.body).messages.at(-1).content, /Emit only tool_calls/);
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', content: null, tool_calls: [{ function: { name: 'write_file', arguments: 'garbage' } }],
      } }] });
    },
  });
  assert.equal(calls, 2);
  assert.equal(result.mode, 'stub');
  assert.match(result.error, /tool-call error after one retry/);
  assert.match(readFileSync(result.taskPath, 'utf8'), /Planning failure/);
  assert.equal(readFileSync(result.recipePath, 'utf8'), result.recipe);
  assert.equal(JSON.parse(readFileSync(join(repoRoot, '.roster', 'memory', 'planner.jsonl'), 'utf8')).status, 'failed');
});

test('written TASK validation rejects a changed Ask, protected paths, and routed-model drift', async (t) => {
  const ask = 'Update README.md.';
  const task = planStub(ask, { reference: 'issue:42', title: 'Update README' }).task;
  for (const [content, expected] of [
    [task.replace('## Ask\nUpdate README.md.', '## Ask\nA different task.'), /unchanged Ask/],
    [task.replace('- `README.md`', '- `.github/workflows/ci.yml`'), /protected files/],
    [task.replace(/^model:.*$/m, 'model: other-model'), /selected fleet model/],
  ]) {
    const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-invalid-draft-'));
    t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
    const worktree = join(repoRoot, 'worktree');
    mkdirSync(worktree);
    let calls = 0;
    await assert.rejects(runPlanner({
      worktree, repoRoot, issue: { number: 42, title: 'Update README', body: ask },
      config: llmConfig, lockedModel: 'test-model', env: {},
      fetchImpl: async () => {
        calls += 1;
        return Response.json({ choices: [calls === 1 ? { finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'task', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content }),
          } }],
        } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Ready.' } }] });
      },
    }), expected);
    assert.equal(calls, 2);
    assert.equal(existsSync(join(worktree, 'RECIPE.yml')), false);
    assert.equal(existsSync(join(worktree, 'ESTIMATE.md')), false);
    assert.equal(existsSync(join(worktree, '.github')), false);
    assert.equal(JSON.parse(readFileSync(join(repoRoot, '.roster', 'memory', 'planner.jsonl'), 'utf8')).status, 'failed');
  }
});

test('planner denies src writes and reports the error to the model without creating app directories', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  const plan = await runPlanner({
    worktree, repoRoot,
    issue: { number: 42, title: 'Protect the task', body: 'Edit `src/app.mjs`.' },
    config: llmConfig, env: {}, vault: { get: async () => undefined },
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      if (calls === 2) {
        assert.match(JSON.parse(body.messages.at(-1).content).error, /only root RECIPE.yml, TASK.md, and ESTIMATE.md/);
        assert.equal(existsSync(join(worktree, 'src')), false);
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({
            title: 'Protect the task', acceptance_checks: ['node --test exits 0'], files_allowed: ['src/app.mjs'],
          }),
        } }] });
      }
      assert.deepEqual(body.tools.map(({ function: tool }) => tool.name), ['write_file']);
      return { status: 200, json: async () => ({ choices: [{ finish_reason: 'tool_calls',
        message: { role: 'assistant', tool_calls: [{ id: 'write', type: 'function', function: {
          name: 'write_file',
          arguments: JSON.stringify({ path: 'src/app.mjs', content: 'bad' }),
        } }] },
      }] }) };
    },
  });
  assert.equal(calls, 2);
  assert.equal(existsSync(join(worktree, 'src', 'app.mjs')), false);
  assert.equal(existsSync(join(worktree, 'src')), false);
  assert.equal(readFileSync(plan.taskPath, 'utf8'), plan.task);
  assert.equal(JSON.parse(readFileSync(join(repoRoot, '.roster', 'memory', 'planner.jsonl'), 'utf8')).status,
    'llm');
});
