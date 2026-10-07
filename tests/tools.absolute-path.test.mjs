import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createRunLog, readLastRunLog } from '../src/lib/run-log.mjs';
import { absoluteInsideWorktree, createTools, OutsideWorktreeError, ToolUsageError } from '../src/runtime/tools.mjs';

function fixture(context) {
  const parent = mkdtempSync(path.join(tmpdir(), 'roster-abs-'));
  context.after(() => rmSync(parent, { recursive: true, force: true }));
  const worktree = path.join(parent, 'wt');
  mkdirSync(path.join(worktree, 'src'), { recursive: true });
  mkdirSync(path.join(worktree, 'vendor', 'pack'), { recursive: true });
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  writeFileSync(path.join(worktree, 'src', 'a.mjs'), 'export const a = 1;\n');
  return { parent, worktree };
}

const usage = (relative, worktree) => (error) => {
  assert.ok(error instanceof ToolUsageError, `expected ToolUsageError, got ${error?.constructor?.name}`);
  assert.ok(!(error instanceof OutsideWorktreeError));
  assert.equal(error.message, `Use a worktree-relative path: "${relative}" instead of an absolute path.`);
  assert.ok(!error.message.includes(worktree), 'the absolute path must not be echoed');
  return true;
};

test('absoluteInsideWorktree maps only absolute paths inside the root, never vendor or escapes', (context) => {
  const { parent, worktree } = fixture(context);
  assert.equal(absoluteInsideWorktree(path.join(worktree, 'src', 'a.mjs'), worktree), 'src/a.mjs');
  assert.equal(absoluteInsideWorktree(worktree, worktree), '.');
  assert.equal(absoluteInsideWorktree('src/a.mjs', worktree), null);
  assert.equal(absoluteInsideWorktree('../x', worktree), null);
  assert.equal(absoluteInsideWorktree(path.join(parent, 'other.md'), worktree), null);
  assert.equal(absoluteInsideWorktree(path.join(worktree, '..', 'other.md'), worktree), null);
  assert.equal(absoluteInsideWorktree(path.join(worktree, 'vendor', 'pack'), worktree), null);
  if (process.platform === 'win32') {
    assert.equal(absoluteInsideWorktree(path.join(worktree, 'README.md').toUpperCase(), worktree), 'README.MD');
  }
});

test('an absolute write_file inside the worktree is a recoverable correction and writes nothing', async (context) => {
  const { parent, worktree } = fixture(context);
  const tools = await createTools({ worktree, allowedFiles: ['**/*'] });
  const target = path.join(worktree, 'src', 'new.mjs');
  await assert.rejects(tools.write_file({ path: target, content: 'x' }), usage('src/new.mjs', worktree));
  assert.equal(existsSync(target), false);
  assert.equal(existsSync(path.join(parent, 'src', 'new.mjs')), false);
  await assert.rejects(tools.read_file({ path: path.join(worktree, 'README.md') }), usage('README.md', worktree));
  await assert.rejects(tools.edit_file({ path: path.join(worktree, 'src', 'a.mjs'), old_string: '1', new_string: '2' }),
    usage('src/a.mjs', worktree));
  assert.equal(readFileSync(path.join(worktree, 'src', 'a.mjs'), 'utf8'), 'export const a = 1;\n');
  await assert.rejects(tools.list_dir({ path: worktree }), usage('.', worktree));
  await assert.rejects(tools.delete_file({ path: path.join(worktree, 'README.md') }), usage('README.md', worktree));
  assert.equal(existsSync(path.join(worktree, 'README.md')), true);
  const relative = await tools.write_file({ path: 'src/new.mjs', content: 'x' });
  assert.equal(relative.path, 'src/new.mjs');
});

test('the observed tools emit an absolute-inside refusal without logging the absolute path', async (context) => {
  const { worktree } = fixture(context);
  const events = [];
  const tools = await createTools({ worktree, allowedFiles: ['**/*'], onEvent: async (event) => { events.push(event); } });
  await assert.rejects(tools.write_file({ path: path.join(worktree, 'src', 'b.mjs'), content: 'x' }),
    usage('src/b.mjs', worktree));
  await assert.rejects(tools.read_file({ path: path.join(worktree, 'README.md') }), usage('README.md', worktree));
  assert.deepEqual(events, [
    { type: 'tool-refused', name: 'write_file', reason: 'absolute-inside' },
    { type: 'tool-result', name: 'write_file', path: 'src/b.mjs', status: 'denied' },
    { type: 'tool-refused', name: 'read_file', reason: 'absolute-inside' },
    { type: 'tool-result', name: 'read_file', path: 'README.md', status: 'denied' },
  ]);
  assert.ok(!JSON.stringify(events).includes(path.basename(path.dirname(worktree))));
  assert.equal(existsSync(path.join(worktree, 'src', 'b.mjs')), false);
});

test('absolute paths outside the worktree, ".." escapes, and vendor stay fatal OutsideWorktreeError', async (context) => {
  const { parent, worktree } = fixture(context);
  const events = [];
  for (const onEvent of [undefined, async (event) => { events.push(event); }]) {
    const tools = await createTools({ worktree, allowedFiles: ['**/*'], onEvent });
    for (const target of [path.join(parent, 'outside.md'), path.join(worktree, '..', 'outside.md'), '../outside.md',
      'src/../../outside.md', 'vendor/pack/x.mjs', path.join(worktree, 'vendor', 'pack', 'x.mjs')]) {
      await assert.rejects(tools.write_file({ path: target, content: 'x' }), OutsideWorktreeError);
      await assert.rejects(tools.read_file({ path: target }), OutsideWorktreeError);
    }
  }
  assert.equal(existsSync(path.join(parent, 'outside.md')), false);
  assert.ok(events.filter((event) => event.type === 'tool-refused').every((event) => event.reason === undefined));
});

test('the run log renders and allowlists an absolute-inside refusal', async (context) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-abs-log-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  let text = '';
  const options = { repoRoot, session: 'roster-351-coder', env: {}, errorOutput: { write(value) { text += String(value); } } };
  const logger = await createRunLog(options);
  await logger.seat('coder', 'roster-351-coder', { llm: { base_url: '', model: '' } }, async (onEvent) => {
    for (const name of ['write_file', 'edit_file', 'read_file']) {
      await onEvent({ type: 'tool-refused', name, reason: 'absolute-inside' });
      assert.match((await readLastRunLog(options)).lastLine, new RegExp(`seat coder tool refused ${name} absolute-inside$`));
    }
    await onEvent({ type: 'tool-refused', name: 'list_dir' });
    assert.match((await readLastRunLog(options)).lastLine, /seat coder tool refused list_dir outside-worktree$/);
    await onEvent({ type: 'tool-refused', name: 'read_file', reason: '/etc/passwd' });
  });
  assert.equal(text.split('\n').filter((line) => line === 'Refused: use a worktree-relative path.').length, 3);
  assert.equal(text.split('\n').filter((line) => line === 'Refused: outside the worktree.').length, 1);
  assert.doesNotMatch(readFileSync(logger.path, 'utf8'), /passwd/);
});
