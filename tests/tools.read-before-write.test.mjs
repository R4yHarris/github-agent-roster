import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createTools } from '../src/runtime/tools.mjs';

function fixture(context) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-rbw-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true }));
  mkdirSync(path.join(worktree, 'src', 'lib'), { recursive: true });
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  writeFileSync(path.join(worktree, 'src', 'app.mjs'), 'export const app = 1;\n');
  writeFileSync(path.join(worktree, 'src', 'lib', 'history.mjs'), 'export const history = [];\n');
  return worktree;
}

const scope = ['README.md', 'src/app.mjs', 'src/lib/history.mjs', 'src/lib/new.mjs', 'docs/NOTE.md'];

test('write_file over an unread code file is refused until the coder reads it', async (context) => {
  const worktree = fixture(context);
  const tools = await createTools({ worktree, allowedFiles: scope });
  await assert.rejects(tools.write_file({ path: 'src/app.mjs', content: 'export const app = 2;\n' }),
    /Read src\/app\.mjs with read_file before replacing it with write_file; use edit_file/);
  assert.equal(readFileSync(path.join(worktree, 'src', 'app.mjs'), 'utf8'), 'export const app = 1;\n');
  await tools.read_file({ path: 'src/app.mjs' });
  await tools.write_file({ path: 'src/app.mjs', content: 'export const app = 2;\n' });
  assert.equal(readFileSync(path.join(worktree, 'src', 'app.mjs'), 'utf8'), 'export const app = 2;\n');
});

test('edit_file and docs writes need no prior read; a written file may be rewritten', async (context) => {
  const worktree = fixture(context);
  const tools = await createTools({ worktree, allowedFiles: scope });
  await tools.edit_file({ path: 'src/app.mjs', old_string: 'app = 1', new_string: 'app = 3' });
  await tools.write_file({ path: 'src/app.mjs', content: 'export const app = 4;\n' });
  await tools.write_file({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' });
  await tools.write_file({ path: 'docs/NOTE.md', content: 'note\n' });
  assert.equal(readFileSync(path.join(worktree, 'src', 'app.mjs'), 'utf8'), 'export const app = 4;\n');
});

for (const [name, args] of [['search_text', { query: 'history' }], ['glob_files', { pattern: 'src/**' }],
  ['list_dir', { path: 'src/lib' }]]) {
  test(`a new src module is refused until a ${name} search for existing behavior`, async (context) => {
    const worktree = fixture(context);
    const tools = await createTools({ worktree, allowedFiles: scope });
    await assert.rejects(tools.write_file({ path: 'src/lib/new.mjs', content: 'export const n = 1;\n' }),
      /Before creating src\/lib\/new\.mjs, use search_text, glob_files, or list_dir/);
    await tools[name](args);
    await tools.write_file({ path: 'src/lib/new.mjs', content: 'export const n = 1;\n' });
  });
}

test('a one-file or slice-only task may create its planned module without a search', async (context) => {
  const worktree = fixture(context);
  const single = await createTools({ worktree, allowedFiles: ['src/lib/new.mjs'] });
  await single.write_file({ path: 'src/lib/new.mjs', content: 'export const n = 1;\n' });
  const slice = await createTools({ worktree, allowedFiles: ['src/lib/other.mjs', 'src/app.mjs'], sliceReadsOnly: true });
  await slice.write_file({ path: 'src/lib/other.mjs', content: 'export const o = 1;\n' });
});

test('a new src module is refused until every module earlier waves delivered is read', async (context) => {
  const worktree = fixture(context);
  const tools = await createTools({ worktree, allowedFiles: ['src/lib/new.mjs', 'src/app.mjs'], sliceReadsOnly: true,
    requiredReads: ['src/lib/history.mjs'] });
  await assert.rejects(tools.write_file({ path: 'src/lib/new.mjs', content: 'export const n = 1;\n' }),
    /read the modules earlier waves delivered: src\/lib\/history\.mjs\. Import and extend them/);
  // Earlier-wave modules stay readable even when reads are limited to the slice.
  await tools.read_file({ path: 'src/lib/history.mjs' });
  await tools.write_file({ path: 'src/lib/new.mjs', content: "import { history } from './history.mjs';\n" });
  await assert.rejects(createTools({ worktree, allowedFiles: ['x.mjs'], requiredReads: 'src/a.mjs' }),
    /Required reads must be a list/);
});
