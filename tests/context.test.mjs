import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { planStub } from '../src/planner/stub.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { appendMemory } from '../src/runtime/memory.mjs';
import { taskSkillNames } from '../src/runtime/skills.mjs';
import { createTools } from '../src/runtime/tools.mjs';

function fixture(context) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-context-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'), '# Conduct\nStay within scope.\n');
  writeFileSync(path.join(worktree, 'TASK.md'),
    planStub('Update README.md.', { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task.replace(/^skills:.*$/m, 'skills: []'));
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nRead before editing.\n');
  for (const file of ['.env', 'app.pem', 'agent-policy.yml', 'unrelated.txt']) {
    writeFileSync(path.join(worktree, file), `body-of-${file}`);
  }
  return { repoRoot, worktree, memoryPath: path.join(repoRoot, '.roster', 'memory', 'coder.jsonl') };
}

test('packs ordered task-scoped context, forty-line skills and the latest twenty memory entries', async (context) => {
  const options = fixture(context);
  const taskPath = path.join(options.worktree, 'TASK.md');
  writeFileSync(taskPath, readFileSync(taskPath, 'utf8').replace('skills: []', 'skills: [implement-task]'));
  for (const name of ['implement-task', 'unrequested']) {
    const directory = path.join(options.repoRoot, 'skills', name);
    mkdirSync(directory, { recursive: true });
    writeFileSync(path.join(directory, 'SKILL.md'),
      Array.from({ length: 45 }, (_, index) => `${name}-line-${index + 1}`).join('\n'));
  }
  for (let index = 0; index < 25; index += 1) {
    await appendMemory({ file: options.memoryPath, repoRoot: options.repoRoot, record: { index } });
  }
  const result = await loadContext(options);
  assert.equal(readFileSync(result.contextPath, 'utf8'), result.pack);
  assert.ok(result.pack.length <= 8000);
  assert.equal(result.truncated, false);
  const headings = ['## Principal coder:', '## TASK.md', '## AGENTS.md',
    '## Task skills', '## Seat memory', '## Relevant file list'];
  const positions = headings.map((heading) => result.pack.indexOf(heading));
  assert.ok(positions.every((position, index) => position >= 0 &&
    (index === 0 || position > positions[index - 1])));
  assert.match(result.pack, /Acceptance checks/);
  assert.match(result.pack, /Files allowed/);
  assert.match(result.pack, /implement-task-line-40/);
  assert.doesNotMatch(result.pack, /implement-task-line-41|unrequested-line/);
  assert.deepEqual(result.files, ['README.md']);
  assert.equal(result.memory.length, 20);
  assert.equal(JSON.parse(result.memory[0]).index, 5);
  assert.equal(JSON.parse(result.memory.at(-1)).index, 24);
  assert.doesNotMatch(result.pack, /body-of-|unrelated\.txt/);
  await assert.rejects(loadContext(options), /EEXIST/);
  const tools = await createTools({ worktree: options.worktree, allowedFiles: ['**/*'] });
  await assert.rejects(tools.write_file({ path: 'CONTEXT.md', content: 'changed' }), /not allowed/);
});

test('character budget is exact and truncation preserves required instructions and newest memory', async (context) => {
  const options = fixture(context);
  for (let index = 0; index < 20; index += 1) {
    await appendMemory({ file: options.memoryPath, repoRoot: options.repoRoot,
      record: { index, summary: 'x'.repeat(100) } });
  }
  const result = await loadContext({ ...options, config: { seat: { context_chars: 1200 } } });
  assert.ok(result.pack.length <= 1200);
  assert.equal(result.truncated, true);
  assert.match(result.pack, /Omitted by context budget/);
  assert.match(result.pack, /Read before editing/);
  assert.match(result.pack, /"index":19/);
  assert.doesNotMatch(result.pack, /"index":0,/);
  assert.match(result.pack, /## Relevant file list from TASK\.md\n\n- `README\.md`/);
});

test('context refuses undersized budgets, protected scope, and unknown requested skills', async (context) => {
  const options = fixture(context);
  await assert.rejects(loadContext({ ...options, config: { seat: { context_chars: 10 } } }),
    /exceed seat.context_chars/);
  const taskPath = path.join(options.worktree, 'TASK.md');
  const task = readFileSync(taskPath, 'utf8');
  for (const file of ['.env', 'app.pem', 'agent-policy.yml']) {
    writeFileSync(taskPath, task.replace('- `README.md`', `- \`${file}\``));
    await assert.rejects(loadContext(options), /exclude protected files/);
  }
  writeFileSync(taskPath, task.replace('skills: []', 'skills: [missing]'));
  await assert.rejects(loadContext(options), /Unknown task skill: missing/);
});

test('a pack fits at the exact configured character boundary', async (context) => {
  const first = await loadContext(fixture(context));
  const exact = await loadContext({
    ...fixture(context), config: { seat: { context_chars: first.pack.length } },
  });
  assert.equal(exact.pack.length, first.pack.length);
  assert.equal(exact.truncated, false);
  const under = await loadContext({
    ...fixture(context), config: { seat: { context_chars: first.pack.length - 1 } },
  });
  assert.ok(under.pack.length <= first.pack.length - 1);
  assert.equal(under.truncated, true, 'optional conventions truncate before required context is refused');
});

test('prior feedback is required context, redacted, and never silently omitted to meet the budget', async (context) => {
  const result = await loadContext({ ...fixture(context), priorFeedback: 'Retry with password="secret value".', env: {} });
  assert.match(result.pack, /## Prior feedback/);
  assert.doesNotMatch(result.pack, /secret value/);
  await assert.rejects(loadContext({ ...fixture(context),
    priorFeedback: 'Required feedback '.repeat(1000), config: { seat: { context_chars: 8000 } } }),
  /prior feedback.*exceed seat.context_chars/);
});

test('task skill names support inline and block frontmatter without accepting paths', () => {
  assert.deepEqual(taskSkillNames('---\nskills: [implement-task, "run-tests"]\n---\n# Task'), [
    'implement-task', 'run-tests',
  ]);
  assert.deepEqual(taskSkillNames('---\nskills:\n  - implement-task\n  - run-tests\nid: one\n---\n# Task'), [
    'implement-task', 'run-tests',
  ]);
  assert.deepEqual(taskSkillNames('# Task'), []);
  assert.deepEqual(taskSkillNames('---\n---\n# Task'), []);
  for (const source of ['skills: [../outside]', 'skills: [x, x]',
    'skills: []\nskills: [x]', 'skills: not-a-list']) {
    assert.throws(() => taskSkillNames(`---\n${source}\n---\n# Task`), /TASK\.md/);
  }
});
