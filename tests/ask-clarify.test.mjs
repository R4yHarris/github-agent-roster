import assert from 'node:assert/strict';
import test from 'node:test';
import { classifyAsk, clarificationHint } from '../src/planner/classify.mjs';
import { clarificationResult, interpretAmbiguousAsk } from '../src/planner/clarify.mjs';
import { formatAsk, submitAsk } from '../src/lib/ask.mjs';

test('a vague ask gets an interpretation, stated assumptions and a short question set', () => {
  for (const ask of ['Improve things.', 'Fix the bug.', 'Make it better', 'How does the planner work?']) {
    assert.equal(classifyAsk(ask).kind, 'clarify', ask);
    const result = interpretAmbiguousAsk(ask);
    assert.ok(result.interpretation.length > 10, ask);
    assert.ok(result.assumptions.length >= 2, ask);
    assert.ok(result.questions.length >= 1 && result.questions.length <= 3, ask);
    assert.match(result.template, /## Allowed Files/);
    assert.match(result.template, /## Acceptance checks/);
  }
  assert.match(interpretAmbiguousAsk('Fix the bug.').interpretation, /defect/);
  assert.match(interpretAmbiguousAsk('How does the planner work?').interpretation, /no code change/);
  assert.match(interpretAmbiguousAsk('Fix the bug.').questions.join('\n'), /failure/);
});

test('the ready-to-fill template is rejected until files are named, then plans as a slice', () => {
  const { template } = interpretAmbiguousAsk('Improve things.');
  assert.throws(() => classifyAsk(template), /Files allowed entries/);
  const filled = template.replace('<path>', 'README.md').replace('<one observable result>', 'README has a Status heading');
  assert.equal(classifyAsk(filled).kind, 'slice');
});

test('roster ask never dead-ends on a vague ask and never contacts GitHub or guesses scope', async () => {
  const result = await submitAsk('Improve things.', {
    env: {}, runCommand: () => assert.fail('Clarify must not contact GitHub'),
  });
  assert.equal(result.mode, 'clarify');
  assert.equal(result.clarification, clarificationHint);
  assert.deepEqual(Object.keys(clarificationResult('Improve things.')).sort(),
    ['assumptions', 'clarification', 'guidance', 'interpretation', 'questions', 'template']);
  const text = formatAsk(result);
  assert.match(text, /Ask kind: clarify/);
  assert.match(text, /Best-effort reading:/);
  assert.match(text, /Assumptions/);
  assert.match(text, /Questions/);
  assert.match(text, /Ready-to-fill Ask/);
  assert.equal(text.split(clarificationHint).length - 1, 1);
});
