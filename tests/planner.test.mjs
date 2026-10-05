import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { parseRecipe } from '../src/lib/recipe.mjs';
import { planAsk, planStub, renderAsk, renderAssignment, taskFilesAllowed } from '../src/planner/stub.mjs';
import { runPlanner } from '../src/seats/planner.mjs';
import { withFleetProfile } from '../src/lib/fleet.mjs';

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
  assert.match(plan.task, /- The requested behavior in the Ask is implemented/);
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
  const config = { ...llmConfig, llm: { ...llmConfig.llm, model: 'selected-model' },
    planner: { turn_budget: 1 } };
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
    .replace('model: ""', 'model: test-model').replace('turn_budget: 32', 'turn_budget: 1'));
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

test('planner salvages a validated fenced JSON plan beside an incomplete TASK without another call', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-same-turn-json-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot, issue: {
      number: 169, title: 'record a human retrospective from the roster shell',
      body: '## Allowed files\n- src/repl.mjs\n- tests/eval.test.mjs\n\n## Checks\n- node --test exits 0.',
    },
    config: { ...llmConfig, planner: { turn_budget: 1 } }, env: {},
    fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant',
        content: '```json\n{"title":"Record /eval human retrospective for a session","acceptance_checks":["node --test exits 0."],"files_allowed":["src/repl.mjs","tests/eval.test.mjs"],"task_class":"feat","difficulty":3,"estimate_min":45}\n```',
        tool_calls: [{ id: 'draft', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: '# Draft task\n\n## Original Ask\n\nrecord a human retrospective from the roster shell\n' }),
        } }],
      } }] });
    },
  });
  assert.equal(calls, 1);
  assert.equal(result.error, undefined);
  assert.deepEqual(taskFilesAllowed(result.task), ['src/repl.mjs', 'tests/eval.test.mjs']);
  assert.equal(readFileSync(result.taskPath, 'utf8'), result.task);
  assert.equal(readFileSync(result.recipePath, 'utf8'), result.recipe);
  assert.equal(existsSync(join(worktree, 'ESTIMATE.md')), true);
});

test('planner gives the model another turn after writing an incomplete TASK.md', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-heading-repair-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  let repairMessage;
  const result = await runPlanner({
    worktree, repoRoot, issue: { number: 42, title: 'Update README', body: 'Update `README.md`.' },
    config: llmConfig, env: {}, fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      if (calls === 1) return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'draft', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: '# Draft task\n' }),
        } }],
      } }] });
      repairMessage = body.messages.at(-1).content;
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
        title: 'Update README', acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'],
      }) } }] });
    },
  });
  assert.equal(calls, 2);
  assert.match(JSON.parse(repairMessage).error, /Acceptance Checks[\s\S]*complete JSON plan/);
  assert.match(result.task, /## Files allowed/);
});

test('planner stops after one no-progress correction for an invalid written TASK', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-no-progress-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot, issue: { number: 42, title: 'Update README', body: 'Update `README.md`.' },
    config: { ...llmConfig, planner: { turn_budget: 2 } }, env: {}, fetchImpl: async () => {
      calls += 1;
      if (calls === 1) return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'draft', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: '# Draft task\n' }),
        } }],
      } }], model: 'test-model' });
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: 'Planning is complete.',
      } }], model: 'test-model' });
    },
  });
  assert.equal(calls, 3);
  assert.equal(result.mode, 'stub');
  assert.match(result.error, /tool-call error after one retry/);
  assert.match(result.error, /Acceptance Checks/);
});

test('routed planner keeps the selected profile when gateway response metadata names another model', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-served-model-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  const result = await runPlanner({
    worktree, repoRoot, issue: { number: 42, title: 'Update README', body: 'Update `README.md`.' },
    config: { ...llmConfig, planner: { turn_budget: 1 } }, lockedModel: 'selected-model', env: {},
    fetchImpl: async () => Response.json({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
        title: 'Update README', acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'],
      }) } }],
      model: 'different-served-model',
    }),
  });
  assert.match(result.task, /^model: selected-model$/m);
  assert.equal(result.response.model, 'different-served-model');
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

test('planner normalizes an alternate JSON plan written to TASK without a repair call', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-alternate-json-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot,
    issue: { number: 42, title: 'Add route test', body: 'Add `tests/route.test.mjs`.' },
    config: llmConfig, env: {},
    fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'alternate', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: JSON.stringify({
            task: 'Add route test',
            files_allowed: ['tests/route.test.mjs'],
            acceptance_checks: ['node --test tests/route.test.mjs exits 0'],
            task_class: 'test', difficulty: 2, estimate_min: 45,
            steps: ['Add a deterministic test'], notes: 'No network calls.',
          }) }),
        } }],
      } }] });
    },
  });
  assert.equal(calls, 1);
  assert.equal(result.error, undefined);
  assert.match(result.task, /^# Task: Add route test$/m);
  assert.match(result.task, /^task_class: test$/m);
});

test('alternate JSON plans cannot grant tools, conflicting titles, protected paths, or extra file scope', async () => {
  const plan = { task: 'Add route test', files_allowed: ['tests/route.test.mjs'],
    acceptance_checks: ['node --test exits 0'], steps: ['Add a deterministic test'], notes: 'No network.' };
  for (const [changes, expected] of [
    [{ tools: ['run_command'] }, /unsupported task plan/],
    [{ title: 'A different task' }, /unsupported task plan/],
    [{ files_allowed: ['.github/workflows/ci.yml'] }, /protected files/],
    [{ files_allowed: ['src/extra.mjs'] }, /beyond the human Ask/],
  ]) {
    let calls = 0;
    await assert.rejects(planAsk('Add tests/route.test.mjs.', {
      config: { ...llmConfig, planner: { turn_budget: 1 } }, env: {},
      fetchImpl: async () => {
        calls += 1;
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({ ...plan, ...changes }),
        } }] });
      },
    }), expected);
    assert.equal(calls, 1);
  }
});

test('the reported draft then fenced task-alias response finalizes a verified handoff in two calls', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-176-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot, issue: { number: 176, title: 'Add run-summary route/profile test',
      body: 'Add tests/route.test.mjs covering the selected route/profile without leaking secrets or using network or wall-clock timing.' },
    config: { ...llmConfig, planner: { turn_budget: 2 } }, lockedModel: 'test-model', env: {},
    fetchImpl: async (_url, request) => {
      calls += 1;
      if (calls === 2) assert.match(JSON.parse(JSON.parse(request.body).messages.at(-1).content).error,
        /complete JSON plan/);
      return Response.json({ model: 'gateway-reported-model',
        usage: { prompt_tokens: 100, completion_tokens: 20 },
        choices: [calls === 1 ? { finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'draft', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: '# Draft\n' }),
          } }],
        } } : { finish_reason: 'stop', message: {
          role: 'assistant', content: 'Plan written. Stopping here.\n```json\n' + JSON.stringify({
            task: 'Add run-summary route/profile test', files_allowed: ['tests/route.test.mjs'],
            task_class: 'test', difficulty: 2, estimate_min: 45,
            steps: ['Exercise public route summary', 'Assert profile visibility and secret non-leakage'],
            acceptance_checks: ['Selected route/profile is visible without secrets',
              'No network or wall-clock dependency', 'node --test tests/route.test.mjs exits 0'],
            notes: 'Test-only change; no app code modifications.',
          }) + '\n```',
        } }],
      });
    },
  });
  assert.equal(calls, 2);
  assert.equal(result.turns, 2);
  assert.equal(result.error, undefined);
  assert.deepEqual(result.usage, { prompt_tokens: 200, completion_tokens: 40 });
  assert.equal(result.run.metrics.model, 'gateway-reported-model');
  assert.match(result.task, /^model: test-model$/m);
  assert.deepEqual(taskFilesAllowed(result.task), ['tests/route.test.mjs']);
  assert.equal(readFileSync(result.taskPath, 'utf8'), result.task);
  assert.equal(readFileSync(result.recipePath, 'utf8'), result.recipe);
  assert.doesNotMatch(result.task, /Planning failure/);
});

test('planner retries one short endpoint timeout before failing the run', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-timeout-retry-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot,
    issue: { number: 42, title: 'Update README', body: 'Update `README.md`.' },
    config: { ...llmConfig, llm: { ...llmConfig.llm,
      base_url: 'https://example.test/v1', request_timeout_ms: 10 } },
    env: {},
    fetchImpl: async () => {
      calls += 1;
      if (calls === 1) return new Promise(() => {});
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: JSON.stringify({
          title: 'Update README',
          acceptance_checks: ['node --test exits 0'],
          files_allowed: ['README.md'],
        }),
      } }] });
    },
  });
  assert.equal(calls, 2);
  assert.equal(result.error, undefined);
});

test('planner timeout recovery aborts the first request, logs one retry, and never makes a third attempt', async (t) => {
  t.mock.timers.enable(['setTimeout', 'setInterval']);
  const requests = [];
  const events = [];
  let firstStarted;
  let secondStarted;
  const first = new Promise((resolve) => { firstStarted = resolve; });
  const second = new Promise((resolve) => { secondStarted = resolve; });
  const pending = planAsk('Update README.md.', {
    config: { ...llmConfig, llm: { ...llmConfig.llm, request_timeout_ms: 120_000 } },
    env: {}, onEvent: (event) => events.push(event),
    fetchImpl: async (_url, request) => {
      requests.push(request);
      (requests.length === 1 ? firstStarted : secondStarted)();
      return new Promise(() => {});
    },
  });
  const rejected = assert.rejects(pending, (error) => error.code === 'ROSTER_LLM_TIMEOUT');
  await first;
  t.mock.timers.tick(120_000);
  await second;
  assert.equal(requests[0].signal.aborted, true);
  assert.equal(requests[1].body, requests[0].body);
  t.mock.timers.tick(120_000);
  await rejected;
  assert.equal(requests.length, 2);
  assert.equal(requests[1].signal.aborted, true);
  assert.deepEqual(events.filter(({ type }) => type === 'timeout-retry'),
    [{ type: 'timeout-retry', attempt: 1, budget: 1 }]);
});

test('a routed public gateway honors its 20-minute allowance and does not abort at the cloud default', async (t) => {
  t.mock.timers.enable(['setTimeout', 'setInterval']);
  const config = withFleetProfile(llmConfig, {
    id: 'gateway', base_url: 'https://gpu.example.test/v1', model: 'selected-model', provider: 'vllm',
    context_max: 262144, concurrency: 1, hardware: 'gpu', notes: '',
    request_timeout_ms: 1_200_000,
  });
  let started;
  let reply;
  let requestSignal;
  let calls = 0;
  const ready = new Promise((resolve) => { started = resolve; });
  const pending = planAsk('Update README.md.', {
    config, lockedModel: 'selected-model', env: {},
    fetchImpl: async (_url, request) => {
      calls += 1;
      requestSignal = request.signal;
      started();
      return new Promise((resolve) => { reply = resolve; });
    },
  });
  await ready;
  t.mock.timers.tick(900_000);
  assert.equal(requestSignal.aborted, false, 'A public cold-inference gateway must survive a 15-minute warmup');
  reply(Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
    title: 'Update README', acceptance_checks: ['README is updated'], files_allowed: ['README.md'],
  }) } }] }));
  const result = await pending;
  assert.equal(calls, 1);
  assert.match(result.task, /^model: selected-model$/m);
});

test('a full cold-start deadline is terminal and does not spend another 20 minutes retrying', async (t) => {
  t.mock.timers.enable(['setTimeout', 'setInterval']);
  let started;
  let calls = 0;
  const ready = new Promise((resolve) => { started = resolve; });
  const pending = planAsk('Update README.md.', {
    config: { ...llmConfig, llm: { ...llmConfig.llm, request_timeout_ms: 1_200_000 } }, env: {},
    fetchImpl: async () => { calls += 1; started(); return new Promise(() => {}); },
  });
  const rejected = assert.rejects(pending, (error) => error.code === 'ROSTER_LLM_TIMEOUT');
  await ready;
  t.mock.timers.tick(1_200_000);
  await rejected;
  assert.equal(calls, 1);
});

test('cancellation during timeout recovery stops before a second HTTP request', async (t) => {
  t.mock.timers.enable(['setTimeout', 'setInterval']);
  const controller = new AbortController();
  let started;
  let calls = 0;
  const ready = new Promise((resolve) => { started = resolve; });
  const pending = planAsk('Update README.md.', {
    config: { ...llmConfig, llm: { ...llmConfig.llm, request_timeout_ms: 120_000 } },
    env: {}, signal: controller.signal,
    onEvent: (event) => { if (event.type === 'timeout-retry') controller.abort(); },
    fetchImpl: async () => { calls += 1; started(); return new Promise(() => {}); },
  });
  const rejected = assert.rejects(pending, (error) => error.code === 'ROSTER_CANCELLED');
  await ready;
  t.mock.timers.tick(120_000);
  await rejected;
  assert.equal(calls, 1);
});

test('planner does not retry authentication or network errors as cold-start timeouts', async () => {
  for (const status of [401, null]) {
    let calls = 0;
    await assert.rejects(planAsk('Update README.md.', {
      config: llmConfig, env: {},
      fetchImpl: async () => {
        calls += 1;
        if (status === null) throw new Error('PRIVATE_NETWORK_ERROR');
        return new Response('PRIVATE_UPSTREAM_BODY', { status });
      },
    }), (error) => !/PRIVATE_/.test(error.message) && error.code !== 'ROSTER_LLM_TIMEOUT');
    assert.equal(calls, 1);
  }
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

test('written TASK validation rejects a changed Ask and protected paths', async (t) => {
  const ask = 'Update README.md.';
  const task = planStub(ask, { reference: 'issue:42', title: 'Update README' }).task;
  for (const [content, expected] of [
    [task.replace('## Ask\nUpdate README.md.', '## Ask\nA different task.'), /unchanged Ask/],
    [task.replace('- `README.md`', '- `.github/workflows/ci.yml`'), /protected files/],
  ]) {
    const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-invalid-draft-'));
    t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
    const worktree = join(repoRoot, 'worktree');
    mkdirSync(worktree);
    let calls = 0;
    await assert.rejects(runPlanner({
      worktree, repoRoot, issue: { number: 42, title: 'Update README', body: ask },
      config: { ...llmConfig, planner: { turn_budget: 1 } }, lockedModel: 'test-model', env: {},
      fetchImpl: async () => {
        calls += 1;
        return Response.json({ choices: [calls === 1 ? { finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'task', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content }),
          } }],
        } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Ready.' } }] });
      },
    }), expected);
    assert.equal(calls, 1);
    assert.equal(existsSync(join(worktree, 'RECIPE.yml')), false);
    assert.equal(existsSync(join(worktree, 'ESTIMATE.md')), false);
    assert.equal(existsSync(join(worktree, '.github')), false);
    assert.equal(JSON.parse(readFileSync(join(repoRoot, '.roster', 'memory', 'planner.jsonl'), 'utf8')).status, 'failed');
  }
});

test('a written TASK cannot silently switch the selected fleet model', async (t) => {
  const ask = 'Update README.md.';
  const task = planStub(ask, { reference: 'issue:42', title: 'Update README' }).task
    .replace(/^model:.*$/m, 'model: other-model');
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-planner-routed-draft-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  await assert.rejects(runPlanner({
    worktree, repoRoot, issue: { number: 42, title: 'Update README', body: ask },
    config: { ...llmConfig, planner: { turn_budget: 1 } }, lockedModel: 'test-model', env: {},
    fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'task', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: task }),
        } }],
      } }] });
    },
  }), /routed planner must keep the selected fleet model/);
  assert.equal(calls, 1);
  assert.match(readFileSync(join(worktree, 'TASK.md'), 'utf8'), /^model: other-model$/m);
  assert.equal(existsSync(join(worktree, 'ESTIMATE.md')), false);
  assert.equal(JSON.parse(readFileSync(join(repoRoot, '.roster', 'memory', 'planner.jsonl'), 'utf8')).status, 'failed');
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
