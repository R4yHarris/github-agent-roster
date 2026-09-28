import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { parseRecipe } from '../src/lib/recipe.mjs';
import { planAsk, planStub, renderAsk, renderAssignment, taskFilesAllowed } from '../src/planner/stub.mjs';

const configExample = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const stubConfig = parseConfig(configExample);
const llmConfig = parseConfig(configExample.replace('base_url: ""', 'base_url: "http://localhost:1234/v1"')
  .replace('model: ""', 'model: test-model'));

test('stub creates an executable single-coder recipe and task without an LLM request', async () => {
  const ask = 'Add a Status section to `README.md`.\n\nKeep the text short.';
  const plan = await planAsk(ask, { config: stubConfig, reference: 'issue:42', title: 'Update project status',
    fetchImpl: () => { throw new Error('stub must not make a network request'); } });
  assert.deepEqual(parseRecipe(plan.recipe), {
    version: 1,
    ask: 'issue:42',
    seats: [{ id: 'coder', principal: 'coder', worker: 'builtin',
      sequence: ['load_context', 'implement', 'run_tests', 'summarize'] }],
  });
  assert.match(plan.task, /^# Task: Update project status/m);
  assert.match(plan.task, /- node --test exits 0/);
  assert.match(plan.task, /## Ask\nAdd a Status section to `README\.md`/);
  assert.deepEqual(taskFilesAllowed(plan.task), ['README.md']);
  assert.equal(plan.usage, null);
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
      ok: true,
      async json() {
        return { choices: [{ message: { content: JSON.stringify({
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
  await assert.rejects(planAsk('Add status to README.', {
    config: llmConfig, fetchImpl: async () => ({ ok: false, status: 401 }),
    env: { ROSTER_API_KEY: 'secret-value' },
  }), (error) => {
    assert.match(error.message, /HTTP 401/);
    assert.ok(!error.message.includes('secret-value'));
    return true;
  });
  await assert.rejects(planAsk('Add status to README.', {
    config: llmConfig, fetchImpl: async () => ({ ok: true, json: async () => ({
      choices: [{ message: { content: JSON.stringify({
        title: 'Bad plan', acceptance_checks: ['test'], files_allowed: ['../secrets'],
      }) } }],
    }) }),
  }), /protected files/);
});
