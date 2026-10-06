import assert from 'node:assert/strict';
import test from 'node:test';
import { coversTestFile, expandTestShards, testCommandFor, testShardBase, verificationDecision } from '../src/runtime/tools.mjs';

test('docs-only work does not run or update tests', () => {
  const decision = verificationDecision(['docs/ENDPOINTS.md']);
  assert.equal(decision.run, false);
  assert.deepEqual(decision.update, []);
  assert.equal(testCommandFor(['docs/ENDPOINTS.md']).skip, true);
  assert.equal(testCommandFor(['README.md']).skip, true);
});

test('a code change runs only the test that covers the changed file', () => {
  const decision = verificationDecision(['src/lib/eval.mjs', 'tests/eval.test.mjs']);
  assert.equal(decision.run, true);
  assert.deepEqual(decision.update, ['tests/eval.test.mjs']);
  assert.deepEqual(testCommandFor(['src/repl.mjs'], 2).args.slice(3), ['tests/repl.test.mjs']);
});

test('a module test split into dotted shards still covers that module', () => {
  assert.equal(testShardBase('tests/builtin.models.test.mjs'), 'tests/builtin.test.mjs');
  assert.equal(testShardBase('tests\\builtin.review-repair.test.mjs'), 'tests/builtin.test.mjs');
  assert.equal(testShardBase('tests/tool-call-continues.test.mjs'), null);
  assert.equal(testShardBase('tests/builtin.test.mjs'), null);
  const update = verificationDecision(['src/lib/builtin.mjs']).update;
  assert.equal(coversTestFile(update, 'tests/builtin.models.test.mjs'), true);
  assert.equal(coversTestFile(update, 'tests/builtin-models.test.mjs'), false);
  assert.equal(coversTestFile(update, 'tests/planner.models.test.mjs'), false);
  const available = ['tests/builtin.models.test.mjs', 'tests/builtin.resume.test.mjs', 'tests/builtin-other.test.mjs',
    'tests/tools.test.mjs', 'tests/helpers.mjs'];
  assert.deepEqual(expandTestShards(['tests/builtin.test.mjs', 'tests/tools.test.mjs', 'tests/missing.test.mjs'], available),
    ['tests/builtin.models.test.mjs', 'tests/builtin.resume.test.mjs', 'tests/tools.test.mjs']);
});
