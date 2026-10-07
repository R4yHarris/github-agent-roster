import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { planStub } from '../src/planner/stub.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { fieldContracts } from '../src/runtime/field-contracts.mjs';

function fixture(context) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-fields-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(path.join(worktree, 'src'), { recursive: true });
  writeFileSync(path.join(worktree, 'src', 'schema.mjs'), [
    'export function validate(record) {',
    "  if (!/^sha256-[a-f0-9]{64}$/.test(record.repoKey)) throw new TypeError('repoKey must be a derived identity hash');",
    '  return record;',
    '}',
    '',
  ].join('\n'));
  writeFileSync(path.join(worktree, 'src', 'store.mjs'), [
    "import { validate } from './schema.mjs';",
    'export const keyOf = (record) => validate(record).repoKey;',
    'export const noteOf = (record) => record.freeNote;',
    '',
  ].join('\n'));
  writeFileSync(path.join(worktree, 'TASK.md'), planStub('Update src/store.mjs to read repoKey.',
    { reference: 'issue:9', metadata: { task_class: 'feat', difficulty: 3 } }).task.replace(/^skills:.*$/m, 'skills: []'));
  for (const name of ['implement-task', 'run-tests', 'read-before-write', 'small-diff', 'result-report']) {
    mkdirSync(path.join(repoRoot, 'skills', name), { recursive: true });
    writeFileSync(path.join(repoRoot, 'skills', name, 'SKILL.md'), `# ${name}\n`);
  }
  return { repoRoot, worktree, memoryPath: path.join(repoRoot, '.roster', 'memory', 'coder.jsonl') };
}

test('field contracts cite validators for fields the slice reads and skip unvalidated fields', async (context) => {
  const { worktree } = fixture(context);
  const contracts = await fieldContracts(worktree, { files: ['src/store.mjs'] });
  assert.match(contracts, /^`repoKey`\n- src\/schema\.mjs:2: .*repoKey must be a derived identity hash/);
  assert.doesNotMatch(contracts, /freeNote/);
  assert.equal(await fieldContracts(worktree, { files: ['README.md'] }), '');
});

test('field contracts rank task-named fields first and stay within the character limit', async (context) => {
  const { worktree } = fixture(context);
  writeFileSync(path.join(worktree, 'src', 'other.mjs'),
    "export const check = (row) => { if (typeof row.ownerId !== 'string') throw new TypeError('ownerId must be a string'); };\n");
  const contracts = await fieldContracts(worktree, { files: ['src/store.mjs'], taskText: 'Keep ownerId stable.' });
  assert.ok(contracts.indexOf('`ownerId`') < contracts.indexOf('`repoKey`'));
  const bounded = await fieldContracts(worktree, { files: ['src/store.mjs'], taskText: 'Keep ownerId stable.', limit: 120 });
  assert.ok(bounded.length <= 120);
  assert.match(bounded, /ownerId/);
});

test('coder context carries field contracts for the allowed source files', async (context) => {
  const options = fixture(context);
  const result = await loadContext(options);
  assert.match(result.pack, /## Field contracts[^\n]*\n+`repoKey`\n- src\/schema\.mjs:2:/);
});
