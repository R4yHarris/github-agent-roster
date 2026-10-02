import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { writeRepoMap } from '../src/lib/repo-map.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { createTools } from '../src/runtime/tools.mjs';
import { formatHelp } from '../src/shell/commands.mjs';

const sourceRoot = fileURLToPath(new URL('../', import.meta.url));
const config = parseConfig(readFileSync(path.join(sourceRoot, 'roster.config.example.yml'), 'utf8'));

function fixture(t, taskClass = 'feat', difficulty = 4) {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-repo-map-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const worktree = path.join(root, 'worktree');
  mkdirSync(worktree);
  for (const name of ['read-before-write', 'small-diff']) {
    cpSync(path.join(sourceRoot, 'skills', name), path.join(root, 'skills', name), { recursive: true });
  }
  writeFileSync(path.join(worktree, 'TASK.md'), planStub('Update README.md.', {
    metadata: { task_class: taskClass, difficulty },
  }).task);
  writeFileSync(path.join(worktree, 'README.md'), 'PRIVATE_FILE_BODY\n');
  mkdirSync(path.join(worktree, 'src', 'deep'), { recursive: true });
  writeFileSync(path.join(worktree, 'src', 'app.mjs'), 'PRIVATE_SOURCE_BODY\n');
  writeFileSync(path.join(worktree, 'src', 'deep', 'nested.mjs'), 'PRIVATE_NESTED_BODY\n');
  return { root, worktree };
}

test('repo map has only names, top two levels and TASK paths and never exceeds 80 lines', async (t) => {
  const { worktree } = fixture(t);
  for (let index = 0; index < 100; index += 1) writeFileSync(path.join(worktree, `file-${index}.txt`), 'PRIVATE_BODY');
  const result = await writeRepoMap({ worktree, env: {} });
  const text = readFileSync(result.path, 'utf8');
  assert.ok(text.trimEnd().split('\n').length <= 80);
  assert.equal(result.lines, text.trimEnd().split('\n').length);
  assert.match(text, /## TASK paths\n- README\.md/);
  assert.doesNotMatch(text, /PRIVATE_|nested\.mjs/);
  assert.match(formatHelp('map'), /80 lines/);
});

test('hard non-docs coder gets the map without widened product reads or map writes', async (t) => {
  const { root, worktree } = fixture(t);
  await writeRepoMap({ worktree, env: {} });
  const context = await loadContext({ worktree, repoRoot: root, config, env: {}, askKind: 'slice' });
  assert.match(context.pack, /Repo map \(filenames only\)/);
  const tools = await createTools({ worktree, allowedFiles: ['README.md'], sliceReadsOnly: true, allowRepoMap: true });
  assert.match(await tools.read_file({ path: '.roster/map.md' }), /src\/app\.mjs/);
  await assert.rejects(tools.read_file({ path: 'src/app.mjs' }), /not allowed/);
  await assert.rejects(tools.write_file({ path: '.roster/map.md', content: 'forged' }), /not allowed/);
});

test('docs at any difficulty and low-difficulty product tasks never load or read the map', async (t) => {
  for (const [taskClass, difficulty] of [['docs', 1], ['docs', 5], ['feat', 2]]) {
    const { root, worktree } = fixture(t, taskClass, difficulty);
    await writeRepoMap({ worktree, env: {} });
    const context = await loadContext({ worktree, repoRoot: root, config, env: {}, askKind: 'slice' });
    assert.doesNotMatch(context.pack, /Repository map|Repo map/);
    const tools = await createTools({ worktree, allowedFiles: ['README.md'], sliceReadsOnly: true });
    await assert.rejects(tools.read_file({ path: '.roster/map.md' }), /refused/);
    await assert.rejects(createTools({ worktree, allowedFiles: ['README.md'], allowRepoMap: true }), /difficulty 4\+/);
  }
});
