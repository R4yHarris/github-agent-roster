import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseConfig, setConfigValue } from '../src/lib/config.mjs';
import { createBuiltinChat } from '../src/lib/llm.mjs';
import { mappedEffort, nextEffort, selectReasoning } from '../src/llm/reasoning.mjs';
import { planOutline } from '../src/planner/plan.mjs';
import { createRunLog } from '../src/lib/run-log.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { buildRun } from '../src/metrics/run.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://192.168.1.48:8888/v1')
  .replace('model: ""', 'model: deepseek-v4.1-flash'));

test('difficulty versus model prior selects effort for every task class and ask kind', () => {
  for (const kind of ['slice', 'feature', 'initiative']) {
    for (const taskClass of ['feat', 'fix', 'docs', 'test']) {
      for (const difficulty of [1, 2, 4, 5]) {
        const selected = selectReasoning(config, { kind, taskClass, difficulty });
        assert.equal(selected.llm.effort, kind === 'slice' && taskClass === 'docs'
          ? 'none' : difficulty <= 2 ? 'l' : 'h');
        assert.equal(selected.llm.model_prior, 'strong');
      }
    }
  }
  for (const kind of ['feature', 'initiative']) {
    const selected = selectReasoning(config, { kind });
    assert.equal(selected.llm.effort, 'l');
    assert.equal(selected.llm.max_tokens, 4096);
  }
  assert.equal(selectReasoning(config, { kind: 'slice', taskClass: 'docs', difficulty: 1 }).llm.max_tokens, 2048);
  const unknown = selectReasoning({ ...config, llm: { ...config.llm, model: 'unrated' } },
    { kind: 'slice', taskClass: 'fix', difficulty: 1 });
  assert.equal(unknown.llm.effort, 'm');
  assert.equal(unknown.llm.model_prior, 'unknown');
  assert.throws(() => selectReasoning(config, { difficulty: 6 }), /difficulty/);
});

test('SGLang DeepSeek4.1 maps medium to high and caps at max; cloud retains four effort tiers', () => {
  const cloud = { ...config.llm, base_url: 'https://api.example.test/v1' };
  assert.deepEqual(['l', 'm', 'h', 'x', 'none'].map((effort) => mappedEffort(config.llm, effort)),
    ['low', 'high', 'high', 'max', 'none']);
  assert.deepEqual(['l', 'm', 'h', 'x', 'none'].map((effort) => mappedEffort(cloud, effort)),
    ['low', 'medium', 'high', 'xhigh', 'none']);
  assert.equal(nextEffort(config.llm, 'l'), 'h');
  assert.equal(nextEffort(config.llm, 'm'), 'x');
  assert.equal(nextEffort(config.llm, 'x'), 'x');
  assert.equal(nextEffort(cloud, 'l'), 'm');
  assert.equal(nextEffort(cloud, 'h'), 'x');
  assert.equal(nextEffort(cloud, 'x'), 'x');
});

test('selected docs request disables reasoning at 2048 and drops reasoning_content from all returned evidence', async () => {
  const selected = selectReasoning(config, { kind: 'slice', taskClass: 'docs', difficulty: 1 });
  const chat = createBuiltinChat(selected, { env: {}, fetchImpl: async (_url, request) => {
    const body = JSON.parse(request.body);
    assert.equal(body.reasoning_effort, 'none');
    assert.equal(body.max_tokens, 2048);
    assert.equal(body.chat_template_kwargs.thinking, false);
    return Response.json({ choices: [{ message: { role: 'assistant', content: 'Done.',
      reasoning_content: 'PRIVATE_THINKING' } }] });
  } });
  const result = await chat({ messages: [{ role: 'user', content: 'Edit README.' }] });
  assert.equal(result.message.reasoning_content, undefined);
  assert.doesNotMatch(JSON.stringify([result, chat.lastResponse]), /PRIVATE_THINKING|reasoning_content/);
});

test('feature planner request uses the strong model prior with low/4096 without implementation tools', async () => {
  await planOutline('Implement a feature.', { config, kind: 'feature', env: {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      assert.equal(body.reasoning_effort, 'low');
      assert.equal(body.max_tokens, 4096);
      assert.equal(body.tools, undefined);
      return Response.json({ choices: [{ message: { role: 'assistant', content: JSON.stringify({
        outcomes: ['Outcome'], issues: [1, 2].map((wave) => ({ title: `Slice ${wave}`, outcome: 'Outcome',
          acceptance_checks: ['Verified'], wave })),
      }) } }] });
    },
  });
});

test('/effort x persists but docs slices disable reasoning; none disables thinking', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-reasoning-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  writeFileSync(join(repoRoot, 'roster.config.example.yml'), example);
  const updated = await setConfigValue('effort', 'x', { repoRoot });
  assert.equal(updated.llm.effort_override, 'x');
  const selected = selectReasoning({ ...updated, llm: { ...updated.llm, base_url: config.llm.base_url,
    model: config.llm.model } }, { kind: 'slice', taskClass: 'docs', difficulty: 1, previousEffort: 'l' });
  assert.equal(selected.llm.effort, 'none');
  assert.equal(mappedEffort(selected.llm), 'none');
  const disabled = await setConfigValue('effort', 'none', { repoRoot });
  assert.equal(buildRun({ config: { ...disabled, llm: { ...disabled.llm, model: 'served' } },
    response: null, env: {} }).env.AI_EFFORT, '-');
  const chat = createBuiltinChat({ ...disabled, llm: { ...disabled.llm, base_url: config.llm.base_url,
    model: config.llm.model } }, { env: {}, fetchImpl: async (_url, request) => {
    const body = JSON.parse(request.body);
    assert.equal(body.reasoning_effort, 'none');
    assert.equal(body.chat_template_kwargs.thinking, false);
    return Response.json({ choices: [{ message: { role: 'assistant', content: 'Done.' } }] });
  } });
  await chat({ messages: [{ role: 'user', content: 'Task' }] });
});

test('shell /effort x remains explicit but docs slices still disable reasoning', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-shell-effort-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  writeFileSync(join(repoRoot, 'roster.config.example.yml'), example);
  const shell = createDispatcher({ repoRoot, cwd: repoRoot, config, env: {},
    output: { write() {} }, errorOutput: { write() {} }, services: {
      repositoryRoot: () => repoRoot,
      setConfigValue: (field, value) => setConfigValue(field, value, { repoRoot }),
      runBuiltinIssue: async (_issue, options) => {
        assert.equal(options.config.llm.effort_override, 'x');
        const selected = selectReasoning(options.config, { kind: 'slice', taskClass: 'docs', difficulty: 1 });
        assert.equal(mappedEffort(selected.llm), 'none');
        return { issue: { number: 108 }, command: null };
      },
    },
  });
  await shell.dispatch('/effort x');
  await shell.dispatch('/run 108');
});
test('human coder request status records chosen effort without exposing reasoning text', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-effort-log-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  let stderr = '';
  const logger = await createRunLog({ repoRoot, session: 'effort-test', env: {},
    errorOutput: { write(line) { stderr += line; } } });
  await logger.seat('coder', 'effort-test', config, async (onEvent) => {
    await onEvent({ type: 'http', phase: 'start', effort: 'low', modelPrior: 'strong',
      reasoning_content: 'PRIVATE_THINKING' });
    return {};
  });
  assert.match(stderr, /^Drafting at low effort\. Model prior: strong\.$/m);
  assert.doesNotMatch(stderr, /PRIVATE_THINKING/);
});

test('difficulty 1 sends low and difficulty 5 sends high for a strong coder, independent of filenames', async () => {
  for (const difficulty of [1, 5]) {
    const selected = selectReasoning(config, { kind: 'slice', taskClass: 'fix', difficulty });
    const chat = createBuiltinChat(selected, { env: {}, fetchImpl: async (_url, request) => {
      assert.equal(JSON.parse(request.body).reasoning_effort, difficulty === 1 ? 'low' : 'high');
      return Response.json({ choices: [{ message: { role: 'assistant', content: 'Done.' } }] });
    } });
    await chat({ messages: [{ role: 'user', content: 'Fix src/widget.mjs.' }] });
  }
  for (const difficulty of [1, 2, 3, 4, 5]) {
    const selected = selectReasoning(config,
      { kind: 'slice', taskClass: 'docs', difficulty, previousEffort: 'x' });
    assert.equal(mappedEffort(selected.llm), 'none');
  }
});

test('catalog overlays change the model prior without fabricating measured or configured context', () => {
  const selected = selectReasoning({ ...config, capabilities: { capabilities: [{
    model_id: config.llm.model, task_class: 'fix', suggested_difficulty: 1,
    context_max: 1048576, concurrency: 1, notes: 'Operator starting guess.',
  }] } }, { kind: 'slice', taskClass: 'fix', difficulty: 2 });
  assert.equal(selected.llm.model_prior, 'limited');
  assert.equal(mappedEffort(selected.llm), 'high');
  assert.equal(selected.llm.context_max, config.llm.context_max);
  const inherited = selectReasoning(selectReasoning(config,
    { kind: 'initiative', taskClass: 'feat', difficulty: 5 }), { kind: 'initiative' });
  assert.equal(mappedEffort(inherited.llm), 'high');
  assert.equal(inherited.llm.task_difficulty, 5);
});
