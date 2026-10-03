import assert from 'node:assert/strict';
import test from 'node:test';
import { createTranscript } from '../src/shell/transcript.mjs';

test('narration deltas append instead of replacing a clipped line', () => {
  const writes = [];
  const transcript = createTranscript({ write: (text, options) => writes.push({ text, options }), color: false });
  transcript.stream('I will read the task and the current README.');
  transcript.stream('I will read the task and the current README. The existing Status section stays.');
  assert.equal(writes.length, 2);
  assert.equal(writes[0].options, undefined);
  assert.equal(writes[1].options, undefined);
  assert.match(writes[0].text, /I will read the task and the current README\.\n$/);
  assert.match(writes[1].text, /The existing Status section stays\.\n$/);
  assert.doesNotMatch(writes[1].text, /I will read the task/);
});
