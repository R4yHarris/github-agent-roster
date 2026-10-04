import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planFromTask, planStub } from '../src/planner/stub.mjs';
import { parseTaskDocument, taskFilesAllowed } from '../src/planner/task.mjs';
import { readTaskMetadata, updateTaskMetadata } from '../src/runtime/estimate.mjs';
import { runPlanner } from '../src/seats/planner.mjs';

const task = readFileSync(new URL('./fixtures/planner-task-92.md', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const ask = 'Add a one-line Status section to README.md';
const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:8000/v1')
  .replace('model: ""', 'model: served-model'));

test('the actual issue-92 TASK fixture validates with Original Ask, Scope, Acceptance Checks and Allowed Files', () => {
  const parsed = parseTaskDocument(task, { expectedAsk: ask });
  assert.equal(parsed.title, ask);
  assert.equal(parsed.ask, ask);
  assert.equal(parsed.acceptance_checks.length, 5);
  assert.deepEqual(parsed.files_allowed, ['README.md']);
  const plan = planFromTask(task, ask);
  assert.equal(plan.difficulty, 1);
  assert.equal(plan.estimate_min, 8);
  assert.equal(plan.task_class, 'docs');
});

test('a missing Allowed Files section fails instead of using paths found in Scope or the Ask', () => {
  const missing = task.replace(/## Allowed Files\n[\s\S]*?(?=## Metadata)/, '');
  assert.throws(() => parseTaskDocument(missing, { expectedAsk: ask }), /Allowed Files/);
  assert.throws(() => taskFilesAllowed(missing), /Allowed Files/);
});

test('heading aliases and case are accepted without requiring Task: on the title', () => {
  for (const acceptance of ['aCcEpTaNcE cHeCkS', 'acceptance_checks']) {
    const aliased = task.replace('# Task: ', '# ').replace('## Original Ask', '## aSK')
      .replace('## Acceptance Checks', `## ${acceptance}`).replace('## Allowed Files', '## fIlEs AlLoWeD')
      .replace(/\n/g, '\r\n');
    assert.deepEqual(parseTaskDocument(aliased, { expectedAsk: ask }).files_allowed, ['README.md']);
    assert.equal(readTaskMetadata(aliased).estimate_min, 8);
  }
});

test('Original Ask can follow the checks/files while metadata after it remains task metadata', () => {
  const source = `# ${ask}\n\n## acceptance_checks\n- node --test exits 0\n\n## Allowed Files\n` +
    `- README.md\n\n## Original Ask\n${ask}\n\n## Metadata\n- task_class: docs\n- difficulty: 1\n- estimate_min: 8\n`;
  assert.deepEqual(parseTaskDocument(source, { expectedAsk: ask }).files_allowed, ['README.md']);
  assert.equal(readTaskMetadata(source).task_class, 'docs');
  assert.equal(readTaskMetadata(source).estimate_min, 8);
});

test('the Ask may include surrounding text but must contain the issue text', () => {
  const containing = task.replace(`## Original Ask\n${ask}`, `## Original Ask\nOperator request:\n${ask}\nPreserve the scope.`);
  assert.equal(parseTaskDocument(containing, { expectedAsk: ask }).title, ask);
  assert.throws(() => parseTaskDocument(task, { expectedAsk: 'Edit a different task.' }), /unchanged Ask/);
});

test('canonical Ask sections still retain embedded issue headings and metadata-looking text as Ask data', () => {
  const original = 'Update README.md.\nmodel: ignored-ask-value\n\n## Acceptance checks\n' +
    '- README.md documents the requested update\n\n' +
    '## Files allowed\n- `README.md`';
  const canonical = planStub(original, { reference: 'issue:92' }).task;
  assert.equal(parseTaskDocument(canonical, { expectedAsk: original }).ask, original);
  assert.equal(readTaskMetadata(canonical).model, '');
});

test('metadata updates keep Scope and Original Ask without leaving duplicate metadata declarations', () => {
  const updated = updateTaskMetadata(task, { estimate_min: 12, model: 'served-model' });
  assert.equal(readTaskMetadata(updated).estimate_min, 12);
  assert.equal(readTaskMetadata(updated).model, 'served-model');
  assert.equal(readTaskMetadata(updated).difficulty, 1);
  assert.match(updated, /## Scope[\s\S]*No other files, sections, or wording changes are in scope/);
  assert.equal(parseTaskDocument(updated, { expectedAsk: ask }).ask, ask);
});

test('ambiguous duplicate headings and protected Allowed Files are rejected', () => {
  assert.throws(() => parseTaskDocument(task.replace('## Metadata', '## allowed_files\n- `src/**`\n\n## Metadata')),
    /duplicate files allowed/);
  assert.throws(() => parseTaskDocument(task.replace('## Allowed Files\n- `README.md`',
    '## Allowed Files\n- `.github/workflows/ci.yml`')), /protected files/);
});

test('writing a complete fixture TASK finishes in the first model response even with a two-turn budget', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-complete-task-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  let calls = 0;
  const result = await runPlanner({
    worktree, repoRoot, config, env: {}, issue: { number: 92, title: ask, body: ask },
    fetchImpl: async () => {
      calls += 1;
      assert.equal(calls, 1, 'A valid written task must not ask the model for confirmation');
      return Response.json({
        model: 'actual-planner-model', usage: { prompt_tokens: 100, completion_tokens: 40 },
        choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{
          id: 'task', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: task }),
          },
        }] } }],
      });
    },
  });
  assert.equal(calls, 1);
  assert.equal(result.turns, 1);
  assert.equal(result.run.metrics.prompt_tokens, 100);
  assert.equal(result.run.metrics.completion_tokens, 40);
  assert.equal(result.metadata.estimate_min, 8);
  assert.match(readFileSync(result.taskPath, 'utf8'), /## Original Ask[\s\S]*## Scope[\s\S]*## Allowed Files/);
  assert.equal(parseTaskDocument(result.task, { expectedAsk: ask }).ask, ask);
});
