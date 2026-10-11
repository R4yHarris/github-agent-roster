import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runLoop } from '../src/runtime/loop.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const llmConfig = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model'));

async function firstFeedback(priorWrites, classification = {}) {
  const task = planStub('Update README.md.').task.replace('- `README.md`', '- `src/a.mjs`');
  let feedback;
  let turns = 0;
  await runLoop({
    config: { ...llmConfig, seat: { ...llmConfig.seat, turn_budget: 1000 } },
    context: { task, pack: task }, env: {}, priorWrites,
    tools: {
      run_test: async () => ({ exit_code: 1, stdout: 'not ok 1 - tests/a.test.mjs\nSyntaxError: Invalid left-hand side',
        stderr: '', failing_files: ['tests/a.test.mjs'], ...classification }),
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

test('without measured baseline evidence an outside failure stays unresolved (#449)', async () => {
  const feedback = await firstFeedback([]);
  assert.match(feedback, /Repair 1 of/);
  assert.doesNotMatch(feedback, /are pre-existing|Report it as pre-existing/);
});

test('only measured pre-existing failures may be excused; prior writes override them', async () => {
  const classification = { preexisting_files: ['tests/a.test.mjs'] };
  assert.match(await firstFeedback([], classification), /outside Allowed Files and are pre-existing/);
  assert.match(await firstFeedback(['tests/a.test.mjs'], classification), /Repair 1 of/);
});

test('explicit unclassified evidence overrides conflicting baseline claims', async () => {
  const feedback = await firstFeedback([], { preexisting_files: ['tests/a.test.mjs'], unclassified_files: ['tests/a.test.mjs'] });
  assert.match(feedback, /Repair 1 of/);
  assert.doesNotMatch(feedback, /are pre-existing|Report it as pre-existing/);
});

test('test-first missing-module repair retains planned product scope instead of limiting edits to failing tests', async () => {
  const task = planStub('Create src/parser.mjs and tests/parser.test.mjs.').task;
  let turns = 0;
  let runs = 0;
  const writes = [];
  const result = await runLoop({
    config: llmConfig, context: { task, pack: task }, env: {},
    tools: {
      run_test: async () => ++runs === 1
        ? { exit_code: 1, stdout: "ERR_MODULE_NOT_FOUND: Cannot find module 'src/parser.mjs'",
          stderr: '', failing_files: ['tests/parser.test.mjs'] }
        : { exit_code: 0, stdout: '# pass 1\n# fail 0', stderr: '' },
      write_file: async ({ path }) => {
        writes.push(path);
        return { path, bytes: 1 };
      },
    },
    fetchImpl: async (_url, request) => {
      turns += 1;
      if (turns === 2) {
        const feedback = JSON.parse(request.body).messages.at(-1).content;
        assert.match(feedback, /Failing test paths \(evidence, not the complete edit scope\): tests\/parser.test.mjs/);
        assert.match(feedback, /missing required module.*implement.*TASK-allowed product path/);
        assert.doesNotMatch(feedback, /Repair only TASK-allowed files: tests\/parser.test.mjs/);
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'implementation', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'src/parser.mjs', content: 'export const parse = () => [];' }),
          } }],
        } }] });
      }
      return Response.json({ choices: [{ finish_reason: turns === 1 ? 'tool_calls' : 'stop', message: {
        role: 'assistant', ...(turns === 1 ? { tool_calls: [{ id: 'red', type: 'function', function: {
          name: 'run_test', arguments: '{}',
        } }] } : { content: 'Implemented the missing planned module and tests pass.' }),
      } }] });
    },
    verify: () => ({ pass: true, reasons: [] }),
  });
  assert.equal(result.error, undefined);
  assert.deepEqual(writes, ['src/parser.mjs']);
  assert.equal(result.testRepairs, 1);
  assert.equal(result.tests.exit_code, 0);
});
