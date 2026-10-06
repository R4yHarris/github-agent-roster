import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { testShardBase } from '../src/runtime/tools.mjs';

// Deterministic budget: one test file runs in one process, so the slowest file bounds suite wall time.
// Size is the CI-stable proxy; `npm test` also warns on measured time (docs/TESTING.md).
const maxTestFileLines = 1000;
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const testFiles = readdirSync(path.join(root, 'tests')).filter((name) => name.endsWith('.test.mjs'));

function sourceModules(directory = path.join(root, 'src'), found = new Set()) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) sourceModules(path.join(directory, entry.name), found);
    else if (entry.name.endsWith('.mjs')) found.add(entry.name.slice(0, -'.mjs'.length));
  }
  return found;
}

test(`every test file stays under ${maxTestFileLines} lines so the suite keeps splitting across workers`, () => {
  const oversized = testFiles.map((name) => [name, readFileSync(path.join(root, 'tests', name), 'utf8').split('\n').length])
    .filter(([, lines]) => lines > maxTestFileLines);
  assert.deepEqual(oversized, [], 'split oversized files into tests/<module>.<topic>.test.mjs shards');
});

test('every test shard names a source module so harness verification can map it', () => {
  const modules = sourceModules();
  const orphans = testFiles.map((name) => `tests/${name}`).filter((file) => testShardBase(file))
    .filter((file) => !modules.has(testShardBase(file).slice('tests/'.length, -'.test.mjs'.length)));
  assert.deepEqual(orphans, []);
});
