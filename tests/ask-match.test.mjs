import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planFromTask, planStub } from '../src/planner/stub.mjs';
import { normalizeAsk, parseTaskDocument } from '../src/planner/task.mjs';
import { readPlannerHandoff, runPlanner } from '../src/seats/planner.mjs';

const task = readFileSync(new URL('./fixtures/planner-task-92.md', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const title = 'Add a one-line Status section to README.md';
const formatted = 'Add a one-line Status section to `README.md`';
const body = `# Ask\n\n${formatted}\n\n## Task metadata\n\ntask_class: feat\ndifficulty: 2\nestimate_min: 15\n`;
const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:8000/v1')
  .replace('model: ""', 'model: served-model'));

function fixture(t) {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-ask-match-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  return { repoRoot, worktree };
}

test('planner-task-92 fixture accepts an issue title-only body', () => {
  const parsed = parseTaskDocument(task, { issueTitle: title, issueBody: title });
  assert.equal(parsed.ask, title);
  assert.deepEqual(parsed.files_allowed, ['README.md']);
});

test('Ask normalization ignores wrapping backticks, whitespace, issue template headings and metadata', () => {
  assert.equal(normalizeAsk(`<!-- template instructions -->\n# Ask\n\n\`${title}\`\n\n` +
    '## Task metadata\n\ntask_class: feat\n'), title);
  const wrapped = task.replace(`## Original Ask\n${title}`, '## Original Ask\n```\n# Ask\n\n' +
    'Add   a one-line\tStatus section\n to `README.md`\n```\n');
  assert.deepEqual(parseTaskDocument(wrapped, { issueTitle: formatted, issueBody: body }).files_allowed, ['README.md']);
});

test('the issue title alone can match even when its body has a different first line and extra template content', () => {
  assert.equal(parseTaskDocument(task, { expectedAsk: 'Different lead text with more details.',
    issueTitle: formatted, issueBody: '# Ask\n\nDifferent lead text with more details.\n' }).ask, title);
});

test('the first nonempty substantive issue body line can match without requiring the entire body', () => {
  assert.equal(parseTaskDocument(task, { issueTitle: 'A different display title',
    issueBody: `\n# Ask\n\n${formatted}\n\nAdditional acceptance constraints.\n` }).ask, title);
  assert.equal(parseTaskDocument(task, { expectedAsk: body }).ask, title);
});

test('Ask (unchanged) and Original Ask (verbatim) heading annotations are accepted', () => {
  for (const heading of ['Ask (unchanged)', 'Original Ask (verbatim)']) {
    const annotated = task.replace('## Original Ask', `## ${heading}`);
    assert.equal(parseTaskDocument(annotated, { issueTitle: title, issueBody: body }).ask, title);
  }
});

test('empty or template-only Original Ask fails even when title and body anchors exist elsewhere', () => {
  for (const empty of ['', ' \t ', '``', '```\n```', '<!-- empty -->\n# Ask\n']) {
    const missing = task.replace(`## Original Ask\n${title}`, `## Original Ask\n${empty}`);
    assert.throws(() => parseTaskDocument(missing, { issueTitle: title, issueBody: body }), /nonempty Original Ask/);
  }
  const unrelated = task.replace(`## Original Ask\n${title}`, '## Original Ask\nA different request.');
  assert.throws(() => parseTaskDocument(unrelated, { issueTitle: title, issueBody: body }), /unchanged Ask/);
});

test('cached valid files pass normalized Ask validation without making any planner request', async (t) => {
  const { worktree } = fixture(t);
  writeFileSync(join(worktree, 'TASK.md'), task.replace('## Original Ask', '## Ask (unchanged)'));
  writeFileSync(join(worktree, 'RECIPE.yml'), planStub(title, { reference: 'issue:92' }).recipe);
  const cached = await readPlannerHandoff({
    worktree, reference: 'issue:92', ask: body, issueTitle: formatted, issueBody: body,
  });
  assert.equal(cached.plan.reused, true);
  assert.equal(cached.plan.turns, 0);
  assert.equal(cached.plan.run, null);
  const empty = task.replace(`## Original Ask\n${title}`, '## Original Ask\n');
  writeFileSync(join(worktree, 'TASK.md'), empty);
  assert.equal((await readPlannerHandoff({
    worktree, reference: 'issue:92', ask: body, issueTitle: formatted, issueBody: body,
  })).plan, null);
});

test('a normalized complete tool-written task finishes in one response with a title anchor', async (t) => {
  const options = fixture(t);
  let requests = 0;
  const result = await runPlanner({
    ...options, config, env: {}, issue: { number: 92, title: formatted,
      body: '# Ask\n\nDifferent body lead; the title identifies the request.\n' },
    fetchImpl: async () => {
      requests += 1;
      assert.equal(requests, 1);
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'task', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: task }),
        } }],
      } }] });
    },
  });
  assert.equal(requests, 1);
  assert.equal(result.turns, 1);
  assert.equal(planFromTask(result.task, body, { issueTitle: formatted }).title, title);
});
