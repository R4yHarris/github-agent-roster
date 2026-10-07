import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runLoop } from '../src/runtime/loop.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const llmConfig = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model'));

const call = (id, name, args) => ({ id, type: 'function', function: { name, arguments: JSON.stringify(args) } });

test('a saved change clears the forced-write state so a verification read is allowed (#354)', async () => {
  const task = planStub('Update README.md.').task.replace('- `README.md`', '- `src/a.mjs`\n- `src/b.mjs`');
  const executed = [];
  let wrote = false;
  let verifiedAfterWrite = false;
  let turns = 0;
  const result = await runLoop({
    config: { ...llmConfig, seat: { ...llmConfig.seat, turn_budget: 1000 } },
    context: { task, pack: task }, env: {},
    tools: {
      read_file: async ({ path }) => { executed.push(`read ${path}`); if (wrote) verifiedAfterWrite = true; return 'file'; },
      write_file: async ({ path }) => { executed.push(`write ${path}`); wrote = true; return { path }; },
      run_test: async () => ({ exit_code: 0, stdout: 'pass', stderr: '' }),
    },
    fetchImpl: async (_url, request) => {
      turns += 1;
      assert.ok(turns < 30, 'loop must not spin');
      const last = JSON.parse(request.body).messages.at(-1);
      const forced = typeof last.content === 'string' && /Stop reading and searching/.test(last.content);
      let tool_calls;
      if (forced) tool_calls = [call(`w-${turns}`, 'write_file', { path: 'src/a.mjs', content: 'export const a = 1;\n' })];
      else if (!wrote) tool_calls = [call(`r-${turns}`, 'read_file', { path: 'src/a.mjs' })];
      else if (!verifiedAfterWrite) tool_calls = [call(`v-${turns}`, 'read_file', { path: 'src/b.mjs' })];
      if (tool_calls) return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls } }] });
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }] });
    },
    verify: () => ({ pass: true, reasons: [] }),
  });
  assert.doesNotMatch(result.error?.message ?? '', /continued exploring after the bounded exploration budget/);
  assert.ok(wrote, 'forced write happened');
  assert.ok(verifiedAfterWrite, 'post-write verification read executed');
  assert.equal(executed.at(-1), 'read src/b.mjs');
});
