import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runLoop } from '../src/runtime/loop.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const llmConfig = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model'));

async function firstFeedback(priorWrites) {
  const task = planStub('Update README.md.').task.replace('- `README.md`', '- `src/a.mjs`');
  let feedback;
  let turns = 0;
  await runLoop({
    config: { ...llmConfig, seat: { ...llmConfig.seat, turn_budget: 1000 } },
    context: { task, pack: task }, env: {}, priorWrites,
    tools: {
      run_test: async () => ({ exit_code: 1, stdout: 'not ok 1 - tests/a.test.mjs\nSyntaxError: Invalid left-hand side',
        stderr: '', failing_files: ['tests/a.test.mjs'] }),
    },
    fetchImpl: async (_url, request) => {
      turns += 1;
      if (turns === 2) feedback = JSON.parse(request.body).messages.filter((m) => m.role === 'user').at(-1).content;
      if (turns === 1) {
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [
          { id: 't-1', type: 'function', function: { name: 'run_test', arguments: '{}' } }] } }] });
      }
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }] });
    },
    verify: () => ({ pass: true, reasons: [] }),
  });
  return feedback;
}

test('a failing file an earlier coder context wrote is repairable, not excused as pre-existing (#356)', async () => {
  const carried = await firstFeedback(['tests\\a.test.mjs']);
  assert.match(carried, /Repair 1 of \d+/);
  assert.match(carried, /tests\/a\.test\.mjs/);
  assert.doesNotMatch(carried, /outside Allowed Files and are pre-existing/);
});

test('without carried writes, a failing file outside Allowed Files is still treated as baseline (#356)', async () => {
  assert.match(await firstFeedback([]), /outside Allowed Files and are pre-existing: tests\/a\.test\.mjs/);
});
