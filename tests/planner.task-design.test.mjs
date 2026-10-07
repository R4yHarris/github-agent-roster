import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { groundingErrors, renderDesign } from '../src/planner/grounding.mjs';
import { buildPlan, planFromTask, planStub } from '../src/planner/stub.mjs';
import { runPlanner, sliceGrounding } from '../src/seats/planner.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:8000/v1')
  .replace('model: ""', 'model: served-model'));
const title = 'Add clean ops';
const body = 'Plan cleanup operations for stale worktrees.\n\n## Files allowed\n- `src/lib/clean-ops.mjs`\n- `tests/clean-ops.test.mjs`\n';
const files = ['src/lib/clean-ops.mjs', 'tests/clean-ops.test.mjs'];
const design = { extend: [], new_exports: [{ name: 'planCleanOps', file: 'src/lib/clean-ops.mjs' }],
  outline: ['Add `planCleanOps`'], edge_cases: [], out_of_scope: [] };

function worktree(t) {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-task-design-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const dir = join(repoRoot, 'worktree');
  mkdirSync(join(dir, 'src', 'lib'), { recursive: true });
  writeFileSync(join(dir, 'src', 'lib', 'repo-locks.mjs'), 'export function acquireRepoLock() {}\n');
  execFileSync('git', ['init', '-q'], { cwd: dir });
  execFileSync('git', ['add', '-A'], { cwd: dir });
  return { repoRoot, dir };
}

const taskWith = (block) => planStub(body, { reference: 'issue:277', title }).task
  .replace(/## Acceptance checks\n[\s\S]*?(?=\n## )/i, '## Acceptance checks\n- `planCleanOps` returns one op per stale worktree\n- node --test exits 0\n')
  .replace(/## Files allowed/i, `${block}\n## Files allowed`);

test('planFromTask returns the TASK.md Design so declared new exports satisfy grounding', async (t) => {
  const { dir } = worktree(t);
  const grounding = await sliceGrounding(dir, `${title}\n${body}`);
  const declared = taskWith(renderDesign(design));
  const complete = planFromTask(declared, body, { issueTitle: title });
  assert.deepEqual(complete.design.new_exports, design.new_exports);
  const build = (task) => {
    const parsed = planFromTask(task, body, { issueTitle: title });
    return buildPlan(body, { reference: 'issue:277', title, acceptanceChecks: parsed.acceptance_checks,
      filesAllowed: parsed.files_allowed, grounding, design: parsed.design });
  };
  assert.doesNotThrow(() => build(declared));
  assert.throws(() => build(taskWith('')), /`planCleanOps` does not exist[\s\S]*## Design[\s\S]*New exports:/);
  assert.deepEqual(groundingErrors({ checks: complete.acceptance_checks, design: complete.design, filesAllowed: files,
    askText: body, index: grounding.index }), []);
});

test('a planner that writes TASK.md with a Design declaring new exports finishes in one turn', async (t) => {
  const { repoRoot, dir } = worktree(t);
  const task = taskWith(renderDesign(design));
  let calls = 0;
  const result = await runPlanner({
    worktree: dir, repoRoot, issue: { number: 277, title, body }, config, env: {},
    fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant',
        tool_calls: Object.entries({ 'RECIPE.yml': planStub(body, { reference: 'issue:277', title }).recipe,
          'TASK.md': task, 'ESTIMATE.md': '# Estimate\n' }).map(([path, content], index) => ({
          id: `p-${index}`, type: 'function', function: { name: 'write_file', arguments: JSON.stringify({ path, content }) } })),
      } }], model: 'served-model', usage: { prompt_tokens: 10, completion_tokens: 5 } });
    },
  });
  assert.equal(result.error, undefined);
  assert.equal(calls, 1);
  assert.match(result.task, /New exports:\n- `planCleanOps` in `src\/lib\/clean-ops\.mjs`/);
});
