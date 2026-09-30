import assert from 'node:assert/strict';
import { existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createTools, plannerArtifactFiles, plannerToolDefinitions } from '../src/runtime/tools.mjs';

function fixture(t) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-planner-tools-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  return worktree;
}

test('planner exposes only write_file for the three exact root artifact names', async (t) => {
  const worktree = fixture(t);
  const tools = await createTools({ worktree, seat: 'planner', allowedFiles: ['**/*'] });
  assert.deepEqual(Object.keys(tools), ['write_file']);
  assert.deepEqual(plannerToolDefinitions.map(({ function: tool }) => tool.name), ['write_file']);
  assert.deepEqual(plannerToolDefinitions[0].function.parameters.properties.path.enum, plannerArtifactFiles);
  for (const file of plannerArtifactFiles) {
    const draft = `Draft ${file}\n`;
    assert.deepEqual(await tools.write_file({ path: file, content: draft }),
      { path: file, bytes: Buffer.byteLength(draft) });
    assert.equal(readFileSync(path.join(worktree, file), 'utf8'), draft);
    await tools.write_file({ path: file, content: `Final ${file}\n` });
    assert.equal(readFileSync(path.join(worktree, file), 'utf8'), `Final ${file}\n`);
  }
});

test('planner never writes app code, paths outside root, protected files, or other managed artifacts', async (t) => {
  const worktree = fixture(t);
  const tools = await createTools({ worktree, seat: 'planner' });
  for (const file of ['src/app.mjs', 'src/TASK.md', '../TASK.md', '.\\TASK.md',
    'src/../TASK.md', 'TASK.md:stream', 'TASK.md.', path.join(worktree, 'TASK.md'),
    '.env', 'agent-policy.yml', '.github/workflows/ci.yml', 'vendor/github-agent-contracts/x.mjs',
    'REVIEW.md', 'RESULT.md', 'ASSIGNMENT.md', '.roster/evals.jsonl']) {
    await assert.rejects(tools.write_file({ path: file, content: 'bad' }),
      /Planner write_file|relative|inside|ambiguous/, file);
  }
  assert.equal(existsSync(path.join(worktree, 'src')), false);
  assert.equal(existsSync(path.join(worktree, '.github')), false);
  assert.equal(existsSync(path.join(worktree, 'vendor')), false);
  assert.equal(existsSync(path.join(worktree, 'TASK.md')), false);
  await assert.rejects(tools.write_file({ path: 'TASK.md', content: 42 }), /must be text/);
  await assert.rejects(tools.write_file({ path: 'TASK.md', content: 'a'.repeat(65_537) }), /64 KiB/);
  await assert.rejects(tools.write_file({ path: 'TASK.md', content: 'valid', command: 'extra' }), /Tool arguments/);
});

test('planner cannot persist known credentials or private-key material in artifacts', async (t) => {
  const worktree = fixture(t);
  const tools = await createTools({ worktree, seat: 'planner', env: { ROSTER_API_KEY: 'test-only-secret-key' } });
  for (const content of ['Token: test-only-secret-key\n', '-----BEGIN PRIVATE KEY-----\ntest-only-material']) {
    await assert.rejects(tools.write_file({ path: 'TASK.md', content }), /credentials or private keys/);
  }
  assert.equal(existsSync(path.join(worktree, 'TASK.md')), false);
});

test('planner refuses pre-existing files and stale edits to its own drafts', async (t) => {
  const worktree = fixture(t);
  writeFileSync(path.join(worktree, 'TASK.md'), 'Operator-owned task\n');
  const tools = await createTools({ worktree, seat: 'planner' });
  await assert.rejects(tools.write_file({ path: 'TASK.md', content: 'replacement' }), /pre-existing/);
  assert.equal(readFileSync(path.join(worktree, 'TASK.md'), 'utf8'), 'Operator-owned task\n');
  await tools.write_file({ path: 'RECIPE.yml', content: 'original' });
  writeFileSync(path.join(worktree, 'RECIPE.yml'), 'modified');
  await assert.rejects(tools.write_file({ path: 'RECIPE.yml', content: 'replacement' }), /changed outside/);
  assert.equal(readFileSync(path.join(worktree, 'RECIPE.yml'), 'utf8'), 'modified');
});

test('planner refuses hard-linked artifacts without changing either link target', async (t) => {
  const worktree = fixture(t);
  const tools = await createTools({ worktree, seat: 'planner' });
  await tools.write_file({ path: 'TASK.md', content: 'owned draft' });
  const link = path.join(worktree, 'copy.md');
  linkSync(path.join(worktree, 'TASK.md'), link);
  await assert.rejects(tools.write_file({ path: 'TASK.md', content: 'replacement' }), /changed outside/);
  assert.equal(readFileSync(link, 'utf8'), 'owned draft');
  assert.equal(readFileSync(path.join(worktree, 'TASK.md'), 'utf8'), 'owned draft');
});

test('planner rejects a symlink artifact without modifying its outside target', async (t) => {
  const worktree = fixture(t);
  const outside = fixture(t);
  const target = path.join(outside, 'outside.md');
  writeFileSync(target, 'outside');
  try {
    symlinkSync(target, path.join(worktree, 'TASK.md'));
  } catch (error) {
    if (['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) {
      t.skip('Creating symlinks is unavailable on this system.');
      return;
    }
    throw error;
  }
  const tools = await createTools({ worktree, seat: 'planner' });
  await assert.rejects(tools.write_file({ path: 'TASK.md', content: 'replacement' }), /symlinks/);
  assert.equal(readFileSync(target, 'utf8'), 'outside');
});

test('coder scope and read-only reviewer remain unchanged', async (t) => {
  const worktree = fixture(t);
  mkdirSync(path.join(worktree, 'src'));
  const coder = await createTools({ worktree, allowedFiles: ['src/**'] });
  await coder.write_file({ path: 'src/app.mjs', content: 'export const ready = true;\n' });
  assert.equal(readFileSync(path.join(worktree, 'src', 'app.mjs'), 'utf8'), 'export const ready = true;\n');
  for (const file of [...plannerArtifactFiles, 'REVIEW.md']) {
    await assert.rejects(coder.write_file({ path: file, content: 'bad' }), /not allowed/);
  }
  await assert.rejects(createTools({ worktree, seat: 'reviewer', allowedFiles: ['**/*'] }),
    /Only planner and coder/);
});
