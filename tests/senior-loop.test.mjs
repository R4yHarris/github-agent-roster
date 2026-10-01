import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { askRequirements, planAsk, planStub, taskFilesAllowed } from '../src/planner/stub.mjs';
import { taskContextPolicy } from '../src/runtime/context-policy.mjs';
import { loadContext } from '../src/runtime/context.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const config = parseConfig(readFileSync(join(root, 'roster.config.example.yml'), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: senior-model'));

test('research and implement-task are opt-in by feat difficulty4+; docs and ordinary tasks stay minimum', () => {
  for (const task_class of ['docs', 'feat', 'fix', 'test']) for (const difficulty of [1, 2, 3, 4, 5]) {
    const task = planStub('Update README.md.', { metadata: { task_class, difficulty } }).task;
    const policy = taskContextPolicy(task);
    const research = task_class === 'feat' && difficulty >= 4;
    assert.equal(policy.research, research);
    assert.equal(policy.minimum, !research);
    if (!research) assert.deepEqual(policy.skills, ['read-before-write', 'small-diff']);
  }
});

test('ordinary team pack is short, includes Ask/outcome/scope/checks, and does not load extra context', async (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-team-pack-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  for (const name of ['read-before-write', 'small-diff']) {
    cpSync(join(root, 'skills', name), join(repoRoot, 'skills', name), { recursive: true });
  }
  const task = planStub('Update README.md.', { metadata: { task_class: 'feat', difficulty: 2 } }).task;
  writeFileSync(join(worktree, 'TASK.md'), task);
  const context = await loadContext({ worktree, repoRoot, config });
  assert.equal(context.contextPolicy.minimum, true);
  assert.deepEqual(context.skills.map(({ name }) => name), ['read-before-write', 'small-diff']);
  assert.match(context.pack, /## Issue Ask[\s\S]*# Outcome:[\s\S]*## Allowed files[\s\S]*## Checks/);
  assert.doesNotMatch(context.pack, /skills: \[|## Principal|## AGENTS|## Seat memory|## Prior feedback/);
  assert.ok(context.pack.length < 3500, `Minimum pack grew to ${context.pack.length} characters`);
});

test('planner scope comes from the human Ask and absent scope cannot become an invented wildcard', async () => {
  assert.throws(() => askRequirements('Build the requested feature.'), /no file scope will be invented/);
  assert.deepEqual(askRequirements('Update README.md.').files, ['README.md']);
  assert.equal(askRequirements('Update README.md.').requiresRun, true);
  assert.equal(askRequirements('Update README.md.\n\n## Allowed Files\n- `README.md`\n').requiresRun, false);
  await assert.rejects(planAsk('Update README.md.', {
    config: { ...config, planner: { turn_budget: 1 } }, env: {}, fetchImpl: async () => Response.json({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
        title: 'Update README', acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md', 'src/extra.mjs'],
      }) } }],
    }),
  }), /cannot invent extra files/);
});

test('scope-inferred planning handoff validates but requires a separate explicit run', async () => {
  const stub = { ...config, llm: { ...config.llm, base_url: '' } };
  const plan = await planAsk('Update README.md.', { config: stub, env: {} });
  assert.equal(plan.requiresRun, true);
  assert.deepEqual(taskFilesAllowed(plan.task), ['README.md']);
  const multiple = await planAsk('Update README.md.\n\n## Outcomes\n- Add Status\n- Rewrite intro\n' +
    '\n## Allowed Files\n- `README.md`\n', { config: stub, env: {} });
  assert.equal(multiple.requiresRun, true);
});
