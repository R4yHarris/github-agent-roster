import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { loadSkills } from '../src/runtime/skills.mjs';

const repoRoot = fileURLToPath(new URL('../', import.meta.url));
const task = (names) => `---\nskills: [${names.join(', ')}]\n---\n# Task: Example\n`;

test('loads implement-task and all five bundled skills with usage, steps and stop conditions', async () => {
  const names = ['implement-task', 'run-tests', 'read-before-write', 'small-diff', 'result-report'];
  const skills = await loadSkills({ repoRoot, task: task(names) });
  assert.deepEqual(skills.map(({ name }) => name), names);
  for (const { content } of skills) {
    assert.match(content, /^## When to use$/m);
    assert.match(content, /^## (Steps|Procedure)$/m);
    assert.match(content, /^## Stop condition$/m);
  }
  await assert.rejects(loadSkills({ repoRoot, task: task(['does-not-exist']) }),
    /Unknown task skill: does-not-exist/);
});

test('only requested skills are opened, in task order, without an implicit fallback', async (context) => {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-selected-skills-'));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  for (const name of ['first', 'second', 'malformed']) {
    mkdirSync(path.join(root, 'skills', name), { recursive: true });
  }
  writeFileSync(path.join(root, 'skills', 'first', 'SKILL.md'), '# First\n');
  writeFileSync(path.join(root, 'skills', 'second', 'SKILL.md'), '# Second\n');
  mkdirSync(path.join(root, 'skills', 'malformed', 'SKILL.md'));
  assert.deepEqual(await loadSkills({ repoRoot: root }), []);
  assert.deepEqual(await loadSkills({ repoRoot: root, task: task(['second', 'first']) }), [
    { name: 'second', content: '# Second\n' }, { name: 'first', content: '# First\n' },
  ]);
  await assert.rejects(loadSkills({ repoRoot: root, task: task(['malformed']) }), /regular SKILL\.md/);
  await assert.rejects(loadSkills({ repoRoot: root, skillsPath: '../outside', task: task(['first']) }),
    /this roster repository/);
  for (const content of ['', 'x'.repeat(65_537)]) {
    writeFileSync(path.join(root, 'skills', 'first', 'SKILL.md'), content);
    await assert.rejects(loadSkills({ repoRoot: root, task: task(['first']) }), /empty|64 KiB/);
  }
});
