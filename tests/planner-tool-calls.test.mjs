import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parsePlannerToolCalls } from '../src/planner/tool-calls.mjs';

const fixture = JSON.parse(readFileSync(new URL('./fixtures/sglang-tool-calls.json', import.meta.url), 'utf8'));

test('SGLang OpenAI function arguments as a JSON string decode into writer arguments', () => {
  const parsed = parsePlannerToolCalls(fixture.choices[0].message);
  assert.equal(parsed.error, undefined);
  assert.deepEqual(parsed.calls[0].args, { path: 'TASK.md', content: '# Draft task\n' });
  assert.equal(typeof parsed.calls[0].function.arguments, 'string');
  assert.equal(parsed.calls[0].id, 'call-sglang-1');
});

test('one JSON tool payload embedded in model text is extracted without interpreting text or quoted braces', () => {
  const payload = { name: 'write_file', arguments: { path: 'TASK.md', content: 'literal { braces } and "quotes"\n' } };
  const parsed = parsePlannerToolCalls({ content: `Draft follows:\n\`\`\`json\n${JSON.stringify(payload)}\n\`\`\``, tool_calls: [] },
    { turn: 2 });
  assert.equal(parsed.error, undefined);
  assert.equal(parsed.fromText, true);
  assert.equal(parsed.calls[0].id, 'planner-2-0');
  assert.deepEqual(parsed.calls[0].args, payload.arguments);
});

test('garbage tool arguments return a clear error without throwing or repeated decoding', () => {
  const call = fixture.choices[0].message.tool_calls[0];
  for (const args of ['garbage', '{"path":', '"double encoded"', '[]', '{}']) {
    let parsed;
    assert.doesNotThrow(() => { parsed = parsePlannerToolCalls({
      tool_calls: [{ ...call, function: { ...call.function, arguments: args } }],
    }); });
    assert.match(parsed.error, /not valid JSON|JSON object with path and content/);
  }
  assert.match(parsePlannerToolCalls({ content: 'tool: {"name":"write_file","arguments":' }).error,
    /incomplete JSON tool payload/);
});

test('unavailable tools and duplicate IDs remain rejected while ordinary plan JSON remains text', () => {
  const call = fixture.choices[0].message.tool_calls[0];
  assert.match(parsePlannerToolCalls({ tool_calls: [call] }, { usedIds: new Set([call.id]) }).error, /duplicated/);
  assert.match(parsePlannerToolCalls({ tool_calls: [{ ...call, function: { ...call.function, name: 'run_test' } }] }).error,
    /only write_file/);
  assert.deepEqual(parsePlannerToolCalls({ content: '{"title":"Plan","acceptance_checks":["node --test exits 0"],"files_allowed":["README.md"]}' }),
    { calls: [], fromText: false });
});

test('an inert empty-argument call is dropped beside real writes but never hides a lone or oversized batch', () => {
  const write = (id, path) => ({ id, type: 'function', function: { name: 'write_file',
    arguments: JSON.stringify({ path, content: 'x' }) } });
  const inert = { id: 'blank', type: 'function', function: { name: 'write_file', arguments: '' } };
  const decoded = parsePlannerToolCalls({ tool_calls: [write('a', 'RECIPE.yml'), inert,
    write('b', 'ESTIMATE.md'), write('c', 'TASK.md')] });
  assert.deepEqual(decoded.calls.map(({ args }) => args.path), ['RECIPE.yml', 'ESTIMATE.md', 'TASK.md']);
  assert.match(parsePlannerToolCalls({ tool_calls: [inert] }).error, /JSON|arguments/);
  assert.match(parsePlannerToolCalls({ tool_calls: [1, 2, 3, 4].map((n) => write('w' + n, 'f' + n)) }).error,
    /at most three/);
});
