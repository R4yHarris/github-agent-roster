import assert from 'node:assert/strict';
import test from 'node:test';
import { classifyAsk } from '../src/planner/classify.mjs';
import { askRequirements, buildPlan, planStub } from '../src/planner/stub.mjs';

const references = [
  'https://example.invalid/reference/guide.md',
  'HTTPS://example.invalid/src/app.mjs?file=tests/app.test.mjs#README.md',
  '<https://example.invalid/guide.md>',
  '[src/app.mjs](https://example.invalid/guide.md)',
  '[src/app.mjs][source]\n\n[source]: https://example.invalid/guide.md',
  '![README.md](https://example.invalid/guide.md)',
  '//example.invalid/src/app.mjs',
  'file:///D:/reference/guide.md',
  'vscode://file/reference/app.mjs',
  'www.example.invalid/src/app.mjs',
  'https://example.invalid/?file="src/app.mjs"',
  '[src/app.mjs](https://example.invalid/ref(part)/guide.md)',
  '[src/app.mjs](https://example.invalid/unclosed/guide.md',
];
for (const reference of references) {
  test('reference is not file authority: ' + reference, () => {
    const ask = 'Improve the product using ' + reference + ' as reference.';
    assert.deepEqual(askRequirements(ask, { allowMissing: true }).files, []);
    assert.equal(classifyAsk(ask).kind, 'clarify');
    assert.throws(() => planStub(ask), /no file scope will be invented/);
    assert.throws(() => buildPlan(ask, { filesAllowed: ['src/app.mjs'], acceptanceChecks: ['Behavior works'] }),
      /no file scope will be invented/);
  });
}

test('local mentions and explicit human scope remain authoritative beside references', () => {
  const link = '[tests/foreign.test.mjs](https://example.invalid/src/foreign.mjs)';
  assert.deepEqual(askRequirements('Update src/app.mjs using ' + link).files, ['src/app.mjs']);
  const ask = 'Use ' + link + '\n\n## Allowed Files\n- src/app.mjs\n- tests/app.test.mjs';
  assert.deepEqual(askRequirements(ask).files, ['src/app.mjs', 'tests/app.test.mjs']);
  assert.throws(() => askRequirements('## Allowed Files\n- https://example.invalid/app.mjs'), /inside the worktree/);
});

test('directory authorization is bounded and cannot be supplied by URL text', () => {
  const ask = 'Write a design using https://example.invalid/docs/guide.md. Output it in a new "notes" directory';
  const scope = askRequirements(ask);
  assert.deepEqual(scope.files, []);
  assert.equal(scope.outputDirectory, 'notes');
  assert.doesNotThrow(() => buildPlan(ask, { reference: 'local:draft', filesAllowed: ['notes/design.md'], acceptanceChecks: ['Document describes the design'] }));
  assert.throws(() => buildPlan(ask, { filesAllowed: ['docs/guide.md'], acceptanceChecks: ['Document describes the design'] }), /inside the granted output directory/);
  assert.equal(classifyAsk('Write a design using [Output it in a new "notes" directory](https://example.invalid/guide.md)').kind, 'clarify');
  const referenceOnly = 'Write a design using https://example.invalid/x?hint=Output.it.in.a.new.notes.directory';
  assert.equal(classifyAsk(referenceOnly).kind, 'clarify');
});

test('scope filtering does not rewrite original reference evidence in the prepared task', () => {
  const ask = 'Update README.md using [reference](https://example.invalid/guide.md).';
  assert.match(planStub(ask).task, /https:\/\/example\.invalid\/guide\.md/);
  assert.deepEqual(askRequirements('Update src/app.mjs using [ref](https://example.invalid/ref(part)/guide.md). Also update tests/app.test.mjs.').files,
    ['src/app.mjs', 'tests/app.test.mjs']);
});
