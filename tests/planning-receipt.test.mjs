import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseRecipe } from '../src/lib/recipe.mjs';
import { validatePlanningReceipt } from '../src/planner/receipt.mjs';
import { parseTaskDocument, taskFilesAllowed } from '../src/planner/task.mjs';
import { readPlannerHandoff } from '../src/seats/planner.mjs';
import { createTools } from '../src/runtime/tools.mjs';

const title = 'Add a one-line Status section to README.md';
const fixture = readFileSync(new URL('./fixtures/planner-task-92.md', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const task = fixture.replace('- `README.md`\n\n## Metadata',
  '- `README.md`\n- `TASK.md`\n- `RECIPE.yml`\n- `ESTIMATE.md`\n\n## Metadata');
const receipt = `title: ${title}\ntask_class: docs\ndifficulty: 1\nestimate_min: 8\nmodel: ""\n` +
  'acceptance_checks:\n  - README.md has a one-line Status section\n  - node --test exits 0\n' +
  'files_allowed:\n  - README.md\n  - TASK.md\n  - ESTIMATE.md\n  - RECIPE.yml\n' +
  'notes: >\n  Only edit README.md. Planning files are harness bookkeeping.\n';

test('planning housekeeping paths do not grant coder writes or invalidate the application scope', async (t) => {
  assert.deepEqual(taskFilesAllowed(task), ['README.md']);
  const worktree = mkdtempSync(join(tmpdir(), 'roster-receipt-tools-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  const tools = await createTools({ worktree, allowedFiles: taskFilesAllowed(task) });
  for (const path of ['TASK.md', 'RECIPE.yml', 'ESTIMATE.md']) {
    await assert.rejects(tools.write_file({ path, content: 'bad' }), /not allowed/);
  }
  assert.throws(() => taskFilesAllowed(task.replace('## Allowed Files\n- `README.md`',
    '## Allowed Files\n- `.env`')), /protected files/);
});

test('a matching planning receipt validates without adding runtime roles or permissions', () => {
  assert.doesNotThrow(() => validatePlanningReceipt(receipt, parseTaskDocument(task, { issueTitle: title })));
  for (const invalid of [receipt + 'grants: merge\n', receipt.replace(title, 'A different task'),
    receipt.replace('files_allowed:\n  - README.md', 'files_allowed:\n  - src/other.mjs'), 'bad yaml']) {
    assert.throws(() => validatePlanningReceipt(invalid, parseTaskDocument(task)));
  }
});

test('a valid cached TASK and receipt bypass planner HTTP and use the fixed builtin recipe', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'roster-cached-receipt-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'worktree'));
  const worktree = join(root, 'worktree');
  writeFileSync(join(worktree, 'TASK.md'), task);
  writeFileSync(join(worktree, 'RECIPE.yml'), receipt);
  const result = await readPlannerHandoff({
    worktree, reference: 'issue:92', ask: title, issueTitle: title, issueBody: `# Ask\n\n${title}\n`,
  });
  assert.equal(result.plan.reused, true);
  assert.equal(result.plan.normalizedRecipe, true);
  assert.equal(result.plan.turns, 0);
  assert.equal(result.plan.task, task);
  const recipe = parseRecipe(result.plan.recipe);
  assert.equal(recipe.ask, 'issue:92');
  assert.deepEqual(recipe.seats.map(({ id, worker }) => ({ id, worker })), [
    { id: 'planner', worker: 'builtin' }, { id: 'coder', worker: 'builtin' }, { id: 'reviewer', worker: 'builtin' },
  ]);
});

test('wrapped numbered acceptance checks remain one check per item', () => {
  const source = task.replace('## Acceptance Checks\n', '## Acceptance Checks\n1. Extra check spanning\n   two lines.\n');
  assert.equal(parseTaskDocument(source).acceptance_checks[0], 'Extra check spanning two lines.');
});
