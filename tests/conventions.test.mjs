import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { planStub } from '../src/planner/stub.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { RULE_LAYERS, conventionsText, deriveConventions } from '../src/runtime/conventions.mjs';

const repo = path.resolve(new URL('..', import.meta.url).pathname.replace(/^\/(\w:)/, '$1'));
const secret = 'sk-live-conventions-0123456789abcdef';

function write(root, file, text) {
  mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
  writeFileSync(path.join(root, file), text);
}

function temp(context, prefix) {
  const root = mkdtempSync(path.join(tmpdir(), prefix));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

test('this repository derives as zero-dependency ESM with node:test shards and kebab-case files', async () => {
  const conventions = await deriveConventions(repo, { useCache: false });
  assert.equal(conventions.languages[0], 'JavaScript');
  assert.match(conventions.moduleSystem, /^ES modules/);
  assert.equal(conventions.sourceExtension, '.mjs');
  assert.equal(conventions.runtimeDependencies, 0);
  assert.equal(conventions.testRunner, 'node:test');
  assert.equal(conventions.testDir, 'tests/');
  assert.equal(conventions.testSuffix, 'test.mjs');
  assert.ok(conventions.shardedTests > 0);
  assert.match(conventions.fileNaming, /^kebab-case/);
  assert.equal(conventions.exportNaming, 'camelCase');
  assert.ok(conventions.core.some(({ file }) => file === 'src/lib/paths.mjs'));
  assert.ok(conventions.core.every(({ file }) => !file.startsWith('vendor/')));
  const text = conventionsText(conventions);
  assert.match(text, /no runtime dependencies \(do not add one\)/);
  assert.match(text, /`<module>\.<topic>\.test\.mjs`/);
  assert.ok(text.length < 1200, `${text.length} chars`);
});

test('a CommonJS jest fixture derives CommonJS, jest, __tests__, camelCase, and its core module', async (context) => {
  const root = temp(context, 'roster-conv-cjs-');
  write(root, 'package.json', JSON.stringify({ name: 'fixture', scripts: { test: 'jest --ci' },
    dependencies: { lodash: '^4' }, devDependencies: { jest: '^29', eslint: '^9' } }));
  write(root, 'yarn.lock', '# lock\n');
  write(root, '.eslintrc.json', '{}\n');
  write(root, 'lib/userStore.js', "const util = require('./sharedUtil');\nexports.findUser = () => util.id();\n");
  write(root, 'lib/orderStore.js', "const util = require('./sharedUtil');\nmodule.exports = { listOrders, saveOrder };\n");
  write(root, 'lib/sharedUtil.js', 'function id() { return 1; }\nmodule.exports = { id, formatName };\n');
  write(root, 'index.js', "const users = require('./lib/userStore');\nmodule.exports = { startServer: users.findUser };\n");
  write(root, '__tests__/userStore.test.js', "const { findUser } = require('../lib/userStore');\ntest('x', () => {});\n");
  const conventions = await deriveConventions(root, { useCache: false });
  assert.deepEqual(conventions.languages, ['JavaScript']);
  assert.match(conventions.moduleSystem, /^CommonJS/);
  assert.equal(conventions.packageManager, 'yarn');
  assert.equal(conventions.runtimeDependencies, 1);
  assert.equal(conventions.testRunner, 'jest');
  assert.equal(conventions.testDir, '__tests__/');
  assert.equal(conventions.testSuffix, 'test.js');
  assert.equal(conventions.fileNaming, 'camelCase (single words lowercase)');
  assert.equal(conventions.exportNaming, 'camelCase');
  assert.deepEqual(conventions.lint, ['ESLint']);
  assert.deepEqual(conventions.core[0], { file: 'lib/sharedUtil.js', importers: 2 });
  assert.match(conventionsText(conventions), /Packages: yarn; 1 runtime dependencies/);
});

test('derivation never reads secrets, policy bodies, or vendor sources', async (context) => {
  const root = temp(context, 'roster-conv-safe-');
  write(root, 'package.json', JSON.stringify({ type: 'module' }));
  write(root, '.env', `API_KEY=${secret}\n`);
  write(root, 'agent-policy.yml', `capabilities:\n  merge: ${secret}\n`);
  write(root, 'vendor/sdk/core.mjs', `export const leaked = '${secret}';\n`);
  write(root, 'node_modules/dep/index.js', "module.exports = require('./x');\n");
  write(root, 'src/app.mjs', "import { leaked } from '../vendor/sdk/core.mjs';\nexport function runApp() { return leaked; }\n");
  const conventions = await deriveConventions(root, { useCache: false });
  const serialized = JSON.stringify(conventions) + conventionsText(conventions);
  assert.doesNotMatch(serialized, new RegExp(secret));
  assert.doesNotMatch(serialized, /vendor|node_modules|agent-policy|\.env/);
  assert.match(conventions.moduleSystem, /^ES modules/);
});

test('the coder pack shows labeled rule layers and conventions without publication rules', async (context) => {
  const repoRoot = temp(context, 'roster-conv-pack-');
  const worktree = path.join(repoRoot, 'worktree');
  write(repoRoot, 'principals/coder.md', '# Conduct\nStay within scope.\n');
  const task = planStub('Update `README.md` with a Status section.',
    { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task.replace(/^skills:.*$/m, 'skills: []');
  write(worktree, 'TASK.md', task);
  write(worktree, 'AGENTS.md', readFileSync(path.join(repo, 'AGENTS.md'), 'utf8'));
  write(worktree, 'README.md', '# Example\n');
  write(worktree, 'package.json', JSON.stringify({ type: 'module', scripts: { test: 'node --test' } }));
  write(worktree, 'src/text-format.mjs', 'export function formatText(value) { return value; }\n');
  write(worktree, 'tests/text-format.test.mjs', "import test from 'node:test';\n");
  const { pack } = await loadContext({ worktree, repoRoot, memoryPath: path.join(repoRoot, 'memory.jsonl'),
    config: { seat: { context_chars: 200000 } } });
  assert.ok(pack.includes(`## Rule layers (precedence)\n\n${RULE_LAYERS}`));
  assert.match(pack, /## Conventions \(derived from this repository\)\n\n- Language: JavaScript; ES modules/);
  assert.match(pack, /Tests: node:test; 1 files under `tests\/`/);
  assert.ok(pack.indexOf('## AGENTS.md') < pack.indexOf('## Rule layers') &&
    pack.indexOf('## Rule layers') < pack.indexOf('## Conventions') &&
    pack.indexOf('## Conventions') < pack.indexOf('## Relevant file list'));
  assert.doesNotMatch(pack, /agent-pr|GITHUB_APP|merge-when-green|gh pr create/);
});
