import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runLoop } from '../src/runtime/loop.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 1000', 'turn_budget: 3'));

for (const name of [undefined, '', '   ']) {
  test(`a missing coder tool name ${JSON.stringify(name)} receives correction without dispatch`, async () => {
    const task = planStub('Update README.md and smoke.test.mjs.').task;
    let turns = 0;
    let dispatched = 0;
    const result = await runLoop({
      config, context: { task, pack: task }, env: {},
      tools: {
        read_file: async () => { dispatched += 1; return { content: 'Existing text' }; },
        run_test: async () => ({ exit_code: 0, stdout: '', stderr: '' }),
      },
      fetchImpl: async (_url, request) => {
        turns += 1;
        if (turns === 2) assert.match(JSON.parse(request.body).messages.at(-1).content, /tool_calls/);
        return Response.json({ choices: [{ finish_reason: turns < 3 ? 'tool_calls' : 'stop',
          message: turns < 3 ? { role: 'assistant', tool_calls: [{
            id: `call-${turns}`, type: 'function', function: turns === 1
              ? { name, arguments: '{}' } : { name: 'read_file', arguments: '{"path":"README.md"}' },
          }] } : { role: 'assistant', content: 'Cannot implement with read-only tools.' } }] });
      },
      verify: () => ({ pass: true, reasons: [] }),
    });
    assert.equal(dispatched, 1);
    assert.equal(turns, 3);
    assert.equal(result.error, undefined);
  });
}

test('inert mixed-batch gateway placeholder is denied visibly without losing valid siblings', async () => {
  const task = planStub('Update README.md and smoke.test.mjs.').task;
  let turns = 0;
  let dispatched = 0;
  const events = [];
  const result = await runLoop({
    config, context: { task, pack: task }, env: {}, onEvent: (event) => events.push(event),
    tools: {
      read_file: async () => { dispatched += 1; return { content: 'Existing text' }; },
      run_test: async () => ({ exit_code: 0, stdout: '', stderr: '' }),
    },
    fetchImpl: async () => {
      turns += 1;
      return Response.json({ choices: [{ finish_reason: turns === 1 ? 'tool_calls' : 'stop',
        message: turns === 1 ? { role: 'assistant', tool_calls: [
          { id: 'inert', type: 'function', function: { name: '', arguments: '' } },
          { id: 'read', type: 'function', function: { name: 'read_file', arguments: '{"path":"README.md"}' } },
        ] } : { role: 'assistant', content: 'Read the existing file.' } }] });
    },
    verify: () => ({ pass: true, reasons: [] }),
  });
  assert.equal(result.error, undefined);
  assert.equal(turns, 2);
  assert.equal(dispatched, 1);
  assert.ok(events.some((event) => event.type === 'tool-result' &&
    event.name === 'empty-gateway-placeholder' && event.status === 'denied'));
});

test('a mixed missing-name call carrying arguments is never discarded or dispatched', async () => {
  const task = planStub('Update README.md and smoke.test.mjs.').task;
  let turns = 0;
  const result = await runLoop({
    config, context: { task, pack: task }, env: {},
    tools: { read_file: async () => assert.fail('Invalid batch dispatched a sibling') },
    fetchImpl: async () => {
      turns += 1;
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant',
        tool_calls: [
          { id: 'bad', type: 'function', function: { name: '', arguments: '{"path":"README.md"}' } },
          { id: 'read', type: 'function', function: { name: 'read_file', arguments: '{"path":"README.md"}' } },
        ],
      } }] });
    },
    verify: () => ({ pass: true, reasons: [] }),
  });
  assert.equal(turns, 2);
  assert.match(result.error.message, /nonempty function name/);
});

test('repeated missing names and withheld tools remain terminal without dispatch', async () => {
  const task = planStub('Update README.md and smoke.test.mjs.').task;
  for (const name of ['', 'list_dir']) {
    let turns = 0;
    const result = await runLoop({
      config: { ...config, seat: { ...config.seat, tools: ['read_file'] } },
      context: { task, pack: task }, env: {},
      tools: { read_file: async () => { assert.fail('Malformed or withheld call was dispatched'); } },
      fetchImpl: async () => {
        turns += 1;
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant',
          tool_calls: [{ type: 'function', function: { name, arguments: '{}' } }],
        } }] });
      },
      verify: () => ({ pass: true, reasons: [] }),
    });
    assert.equal(turns, name ? 1 : 2);
    assert.ok(result.error);
  }
});

test('test runner infrastructure failures stop the loop before another model turn or full suite', async () => {
  const task = planStub('Update README.md and smoke.test.mjs.').task;
  for (const failure of [
    new Error('node --test timed out after 60 seconds'),
    Object.assign(new Error('process could not start'), { code: 'ENOENT' }),
  ]) {
    let turns = 0;
    let tests = 0;
    let verifies = 0;
    const result = await runLoop({
      config, context: { task, pack: task }, env: {},
      tools: { run_test: async () => { tests += 1; throw failure; } },
      fetchImpl: async () => {
        turns += 1;
        return Response.json({ choices: [{ finish_reason: turns === 1 ? 'tool_calls' : 'stop',
          message: turns === 1 ? { role: 'assistant', tool_calls: [{
            id: 'test', type: 'function', function: { name: 'run_test', arguments: '{}' },
          }] } : { role: 'assistant', content: 'Done.' } }] });
      },
      verify: () => { verifies += 1; return { pass: true, reasons: [] }; },
    });
    assert.equal(result.error, failure);
    assert.equal(turns, 1);
    assert.equal(tests, 1);
    assert.equal(verifies, 0);
    assert.equal(result.testRepairs, 0);
  }
});
