import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { planStub } from '../src/planner/stub.mjs';
import { applyReadmeStatus } from '../src/runtime/readme-status.mjs';
import { createTools } from '../src/runtime/tools.mjs';

function fixture(t, readme) {
  const worktree = mkdtempSync(join(tmpdir(), 'roster-readme-status-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  const task = planStub('Add a one-line Status section to README.md.').task;
  writeFileSync(join(worktree, 'TASK.md'), task);
  writeFileSync(join(worktree, 'README.md'), readme);
  return { worktree, task };
}

test('deterministic Status edit is additive, one body line, and preserves LF/CRLF and EOF conventions', async (t) => {
  for (const newline of ['\n', '\r\n']) for (const finalNewline of [true, false]) {
    const original = `# Project${newline}${newline}Intro.${newline}${newline}## Usage${newline}Use it.` +
      (finalNewline ? newline : '');
    const options = fixture(t, original);
    const tools = await createTools({ worktree: options.worktree, allowedFiles: ['README.md'] });
    await applyReadmeStatus({ task: options.task, tools });
    const text = readFileSync(join(options.worktree, 'README.md'), 'utf8');
    assert.ok(text.includes(`## Status${newline}Experimental - APIs may change.${newline}${newline}## Usage`));
    assert.equal(text.endsWith('\n'), finalNewline);
    assert.equal(text.replace(`## Status${newline}Experimental - APIs may change.${newline}${newline}`, ''), original);
    const once = text;
    await applyReadmeStatus({ task: options.task, tools });
    assert.equal(readFileSync(join(options.worktree, 'README.md'), 'utf8'), once);
  }
});

test('fallback cannot invent a Status task, expand scope, rewrite a multiline section, or ignore changed TASK', async (t) => {
  const options = fixture(t, '# Project\n\n## Status\nOne.\nTwo.\n');
  const tools = await createTools({ worktree: options.worktree, allowedFiles: ['README.md'] });
  await assert.rejects(applyReadmeStatus({ task: options.task, tools }), /refusing an automatic rewrite/);
  await assert.rejects(applyReadmeStatus({ task: planStub('Update README.md.').task, tools }), /explicit Status task/);
  await assert.rejects(applyReadmeStatus({ task: planStub('Add a Status section to README.md and src/app.mjs.').task, tools }),
    /scoped only to README/);
  writeFileSync(join(options.worktree, 'TASK.md'), 'changed');
  await assert.rejects(applyReadmeStatus({ task: options.task, tools }), /TASK.md changed/);
});
