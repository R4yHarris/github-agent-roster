import assert from 'node:assert/strict';
import test from 'node:test';
import { testCommandFor, verificationDecision } from '../src/runtime/tools.mjs';

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
