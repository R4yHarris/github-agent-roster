import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runLoop } from '../src/runtime/loop.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 1000', 'turn_budget: 3'));

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
