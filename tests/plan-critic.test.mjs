import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { criticTools, critiquePlan, deterministicPlanDefects } from '../src/planner/critic.mjs';
import { buildPlan } from '../src/planner/stub.mjs';
import { parseTaskDocument } from '../src/planner/task.mjs';
import { critiquePlannerHandoff } from '../src/seats/planner.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { renderDesign } from '../src/planner/grounding.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { readTaskMetadata } from '../src/runtime/estimate.mjs';

const draft = (checks = ['Existing test `existing behavior` exists'], files = ['README.md', 'missing.md']) =>
  `# Task: Improve docs\n\ndifficulty: 2\nestimate_min: 15\ntask_class: docs\nmodel: \n\n` +
  `## Acceptance checks\n\n${checks.map((check) => `- ${check}`).join('\n')}\n\n` +
  `## Files allowed\n\n${files.map((file) => `- \`${file}\``).join('\n')}\n\n` +
  '## Ask\n\nImprove documented behavior in README.md and missing.md.\n';
const index = { files: new Set(['README.md']), symbols: new Set(), exportsByFile: new Map() };

test('deterministic critic flags an already-covered check and an undeclared missing file', () => {
  const defects = deterministicPlanDefects(draft(), { index, testNames: ['existing behavior'] });
  assert.equal(defects.length, 2);
  assert.match(defects[0].problem, /missing.md/);
  assert.equal(defects[1].check, 1);
  assert.match(defects[1].problem, /already-existing/);
});

test('one failing planner revision proceeds with reviewer-visible notes, preserving Ask and scope', async () => {
  const task = draft();
  let attempts = 0;
  const result = await critiquePlan(task, { index, testNames: ['existing behavior'],
    revise: async ({ task: original, defects }) => {
      attempts += 1;
      assert.equal(original, task);
      assert.equal(defects.length, 2);
      return original;
    } });
  assert.equal(attempts, 1);
  assert.equal(result.revised, true);
  assert.equal(result.defects.length, 2);
  assert.match(result.task, /## Critic notes/);
  assert.equal(parseTaskDocument(result.task).ask, parseTaskDocument(task).ask);
  assert.deepEqual(parseTaskDocument(result.task).files_allowed, ['README.md', 'missing.md']);
  const again = await critiquePlan(result.task, { index, testNames: ['existing behavior'] });
  assert.equal(again.task.match(/## Critic notes/g).length, 1);
});

test('a successful revision removes stale notes and does not call the planner again', async () => {
  let revisions = 0;
  const result = await critiquePlan(draft(), { index, testNames: ['existing behavior'],
    revise: async () => { revisions += 1; return draft(['README documents the changed behavior'], ['README.md']); } });
  assert.equal(revisions, 1);
  assert.deepEqual(result.defects, []);
  assert.doesNotMatch(result.task, /Critic notes/);
});

test('critic notes preserve frontmatter, metadata, and Ask in CRLF tasks', async () => {
  const task = buildPlan('Improve README.md and missing.md.', { reference: 'issue:42',
    acceptanceChecks: ['Changed documentation'], filesAllowed: ['README.md', 'missing.md'] }).task;
  const result = await critiquePlan(task.replace(/\n/g, '\r\n'), { index });
  assert.equal(readTaskMetadata(result.task).difficulty, readTaskMetadata(task).difficulty);
  assert.equal(parseTaskDocument(result.task).ask, parseTaskDocument(task).ask);
  assert.ok(result.task.startsWith('---\nskills:'));
  const repeated = await critiquePlan(result.task.replace(/\n/g, '\r\n'), { index });
  assert.equal(repeated.task.match(/## Critic notes/g).length, 1);
});

test('critic profile configuration is explicit and rejects unsafe identifiers', () => {
  const source = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
  const configured = source.replace(/planner:\r?\n/, 'planner:\n  critic_profile: critic-fixture\n');
  assert.equal(parseConfig(configured).planner.critic_profile, 'critic-fixture');
  assert.throws(() => parseConfig(configured.replace('critic-fixture', '../escape')), /critic_profile/);
});

test('critic refuses scope expansion, Ask changes, cancellation, and malformed model output', async () => {
  for (const changed of [draft(['Behavior changed'], ['README.md', 'new.md']),
    draft(['Behavior changed']).replace('Improve documented behavior in README.md and missing.md.', 'A different Ask.')]) {
    await assert.rejects(critiquePlan(draft(), { index, revise: async () => changed }), /Ask|widen/);
  }
  for (const content of ['not JSON', '{"defects":[{"check":99,"problem":"bad","fix":"bad"}]}',
    '{"defects":[],"scope":["src"]}', '{"defects":[{"check":1,"problem":"bad"}]}']) {
    await assert.rejects(critiquePlan(draft(), { index, chat: async () => ({ message: { content } }) }));
  }
  await assert.rejects(critiquePlan(draft(), { signal: AbortSignal.abort() }));
});

test('optional model critic has no tools, a 600-token cap and grounded input', async () => {
  assert.deepEqual(criticTools, []);
  let calls = 0;
  const result = await critiquePlan(draft(['Changed documentation'], ['README.md']), {
    index, definitions: 'real definitions', chat: async (request) => {
      calls += 1;
      assert.deepEqual(request.tools, []);
      assert.equal(request.max_tokens, 600);
      assert.match(request.messages[1].content, /real definitions/);
      return { message: { content: '{"defects":[]}' } };
    },
  });
  assert.equal(calls, 1);
  assert.deepEqual(result.defects, []);
  await assert.rejects(critiquePlan(draft(), { chat: async () => ({
    message: { content: '{"defects":[]}', tool_calls: [{ function: { name: 'write_file' } }] },
  }) }), /cannot request tools/);
});

test('new files may be explicitly declared, and a test name alone is not proof behavior already passes', () => {
  const task = draft(['Extend test `existing behavior` for the new behavior']);
  const declared = task.replace('## Files allowed', '## New files\n\n- `missing.md`\n\n## Files allowed');
  assert.deepEqual(deterministicPlanDefects(declared, { index, testNames: ['existing behavior'] }), []);
  assert.equal(deterministicPlanDefects(draft(), { index, testNames: [] }).length, 1);
});

test('grounding rejects missing symbols and duplicate Design exports', () => {
  const source = buildPlan('Improve src/existing.mjs.', { reference: 'issue:42', title: 'Code',
    acceptanceChecks: ['`missingHelper` is covered'], filesAllowed: ['src/existing.mjs'] }).task;
  const grounded = { files: new Set(['src/existing.mjs']), symbols: new Set(),
    exportsByFile: new Map([['src/existing.mjs', new Set()]]) };
  assert.match(deterministicPlanDefects(source, { index: grounded })[0].problem, /missingHelper/);
  const duplicate = source.replace('## Files allowed', `${renderDesign({
    extend: [], new_exports: [{ file: 'src/existing.mjs', name: 'alreadyExported' }],
    outline: [], edge_cases: [], out_of_scope: [],
  })}\n## Files allowed`);
  grounded.symbols.add('alreadyExported');
  grounded.exportsByFile.set('src/other.mjs', new Set(['alreadyExported']));
  assert.ok(deterministicPlanDefects(duplicate, { index: grounded })
    .some(({ problem }) => /alreadyExported.*already exists/.test(problem)));
});

test('real handoff runs one read-only planner revision and writes notes before coder entry', async (t) => {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-critic-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  writeFileSync(join(cwd, 'README.md'), '# Example\n');
  execFileSync('git', ['init', '--quiet'], { cwd });
  execFileSync('git', ['add', 'README.md'], { cwd });
  const configText = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
  const config = parseConfig(configText.replace('base_url: ""', 'base_url: http://fixture.invalid/v1')
    .replace('model: ""', 'model: fixture-model'));
  const plan = { task: draft(['Changed documentation']) };
  writeFileSync(join(cwd, 'TASK.md'), plan.task);
  writeFileSync(join(cwd, 'ESTIMATE.md'), '# Original estimate\n');
  let requests = 0;
  const result = await critiquePlannerHandoff(plan, { worktree: cwd, learningRoot: cwd,
    ask: 'Improve documented behavior in README.md and missing.md.', title: 'Improve docs', reference: 'issue:42', config, env: {},
    fetchImpl: async (_url, request) => {
      requests += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.tools, undefined);
      assert.match(body.messages[1].content, /Plan critic revision/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ title: 'Improve docs', acceptance_checks: ['Changed documentation'],
          files_allowed: ['README.md', 'missing.md'] }) } }] });
    } });
  assert.equal(requests, 1);
  assert.equal(result.critic.revised, true);
  assert.match(result.task, /## Critic notes/);
  assert.equal(readTaskMetadata(result.task).difficulty, 2);
  assert.equal(parseTaskDocument(result.task).ask, parseTaskDocument(plan.task).ask);
  assert.equal(readFileSync(join(cwd, 'TASK.md'), 'utf8'), result.task);
  assert.equal(readFileSync(join(cwd, 'README.md'), 'utf8'), '# Example\n');
});

test('fleet critic uses only its explicit independent profile and records actual response evidence', async (t) => {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-critic-profile-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  writeFileSync(join(cwd, 'README.md'), '# Example\n');
  execFileSync('git', ['init', '--quiet'], { cwd });
  execFileSync('git', ['add', 'README.md'], { cwd });
  mkdirSync(join(cwd, '.roster'));
  const parsed = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
    .replace('base_url: ""', 'base_url: http://fixture.invalid/v1').replace('model: ""', 'model: planner-model'));
  const config = { ...parsed, planner: { ...parsed.planner, critic_profile: 'critic-fixture' } };
  const profile = { id: 'critic-fixture', base_url: config.llm.base_url, model: 'critic-model', provider: 'vllm',
    context_max: 32768, concurrency: 1, hardware: 'fixture-only', notes: '' };
  const fleetPath = join(cwd, '.roster', 'fleet.yml');
  writeFileSync(fleetPath, formatFleet({ profiles: [profile] }));
  const task = draft(['README documents the change'], ['README.md']);
  const options = { worktree: cwd, learningRoot: cwd, ask: parseTaskDocument(task).ask,
    title: 'Docs', reference: 'issue:42', config, env: {}, vault: { get: async () => undefined } };
  let calls = 0;
  const result = await critiquePlannerHandoff({ task }, { ...options, fetchImpl: async (url, request) => {
    calls += 1;
    assert.equal(url, 'http://fixture.invalid/v1/chat/completions');
    const body = JSON.parse(request.body);
    assert.equal(body.model, 'critic-model');
    assert.equal(body.max_tokens, 600);
    assert.equal(body.tools, undefined);
    return Response.json({ model: 'critic-model', usage: { prompt_tokens: 123, completion_tokens: 9 },
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: '{"defects":[]}' } }] });
  } });
  assert.equal(calls, 1);
  assert.equal(result.critic.runs[0].env.AI_MODEL, 'critic-model');
  assert.equal(result.critic.runs[0].metrics.prompt_tokens, 123);
  assert.deepEqual(result.critic.defects, []);
  for (const profiles of [[], [{ ...profile, model: config.llm.model }]]) {
    writeFileSync(fleetPath, formatFleet({ profiles }));
    await assert.rejects(critiquePlannerHandoff({ task }, { ...options,
      fetchImpl: () => assert.fail('invalid profile must not call an endpoint') }), /profile/);
  }
  writeFileSync(fleetPath, formatFleet({ profiles: [profile] }));
  await assert.rejects(critiquePlannerHandoff({ task }, { ...options, fetchImpl: async () => Response.json({
    choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: '{"defects":[],"unexpected":true}' } }],
  }) }), /defects array/);
});
