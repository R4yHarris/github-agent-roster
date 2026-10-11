import assert from 'node:assert/strict';
import test from 'node:test';
import { parseLeadingJsonObject } from '../src/planner/stub.mjs';

test('leading JSON object tolerates stray trailing prose but not other garbage', () => {
  const body = '{"title":"a } \\" b","files_allowed":["x.md"]}';
  assert.deepEqual(parseLeadingJsonObject(body), JSON.parse(body));
  assert.deepEqual(parseLeadingJsonObject(`${body} •`), JSON.parse(body));
  assert.deepEqual(parseLeadingJsonObject(`${body}\n\nNote: no repo exploration.`), JSON.parse(body));
  assert.throws(() => parseLeadingJsonObject('Plan: {"a":1}'), SyntaxError);
  assert.throws(() => parseLeadingJsonObject('{"a":1'), SyntaxError);
});
