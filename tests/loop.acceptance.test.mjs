import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { acceptanceSourceIdentity } from '../src/runtime/checklist.mjs';
import { runLoop } from '../src/runtime/loop.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const llmConfig = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 1000', 'turn_budget: 3'));

test('one current observation completes individually evidenced checks in a single batch without serial model turns', async () => {
  const task = '# Task: Utility\n\ndifficulty: 2\ntask_class: feat\n\n## Ask\n' +
    'Implement a bounded utility with positive, negative and zero inputs.\n\n## Acceptance checks\n' +
    '- positive input passes\n- negative input passes\n- zero input passes\n\n' +
    '## Files allowed\n- app.mjs\n- app.test.mjs\n';
  const source = acceptanceSourceIdentity(new Map([['app.mjs', 'verified-source']]));
  const config = { ...llmConfig, seat: { ...llmConfig.seat, evidence_workspace: true } };
  let requests = 0;
  let runs = 0;
  const result = await runLoop({
    config, worktree: process.cwd(), context: { task, pack: task },
    acceptanceSource: async () => source,
    verify: async () => ({ pass: true, reasons: [] }),
    tools: { run_test: async () => {
      runs++;
      return { exit_code: 0, stdout: 'positive, negative and zero controls passed', stderr: '' };
    } },
    fetchImpl: async (_url, request) => {
      requests++;
      const body = JSON.parse(request.body);
      if (requests === 1) {
        assert.match(body.messages[1].content, /batch all proven checks as done/);
        const definition = body.tools.find(({ function: fn }) => fn.name === 'update_checklist');
        assert.match(definition.function.description, /individual current evidence/);
      }
      const message = requests === 1 ? { role: 'assistant', tool_calls: [
        { id: 'test', type: 'function', function: { name: 'run_test', arguments: '{}' } },
        { id: 'batch', type: 'function', function: { name: 'update_checklist', arguments: JSON.stringify({
          items: [1, 2, 3].map(id => ({ id, status: 'done', evidence: `node --test: check ${id}, input control passed` })),
        }) } },
      ] } : { role: 'assistant', content: 'All three checks done with recorded tests.' };
      return Response.json({ model: 'local-model', usage: { prompt_tokens: 20, completion_tokens: 5 },
        choices: [{ finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message }] });
    },
  });
  assert.equal(result.error, undefined);
  assert.equal(requests, 2);
  assert.equal(result.turns, 2);
  assert.equal(runs, 2, 'The final mandatory harness verification still runs');
  assert.equal(result.acceptanceContinuation.outcome, 'verified');
  assert.deepEqual(result.acceptanceContinuation.items.map(({ status }) => status), ['done', 'done', 'done']);
  assert.deepEqual(result.acceptanceContinuation.items.map(({ evidenceRef }) => evidenceRef), [1, 1, 1]);
});

test('batch completion remains atomic when one check lacks evidence or two checks claim active work', async (context) => {
  for (const scenario of ['missing-evidence', 'multiple-active']) {
    await context.test(scenario, async () => {
      const task = '# Task: Utility\n\ndifficulty: 2\ntask_class: feat\n\n## Ask\nImplement a utility.\n\n' +
        '## Acceptance checks\n- positive input passes\n- zero input passes\n\n## Files allowed\n- app.mjs\n- app.test.mjs\n';
      const controller = new AbortController();
      let requests = 0;
      const result = await runLoop({
        config: { ...llmConfig, seat: { ...llmConfig.seat, evidence_workspace: true } },
        worktree: process.cwd(), context: { task, pack: task }, signal: controller.signal,
        acceptanceSource: async () => acceptanceSourceIdentity(new Map([['app.mjs', 'bytes']])),
        tools: { run_test: async () => ({ exit_code: 0, stdout: 'pass', stderr: '' }) },
        verify: async () => assert.fail('Denied checklist batch must not reach verified completion'),
        fetchImpl: async (_url, request) => {
          requests++;
          const body = JSON.parse(request.body);
          if (requests === 2) {
            assert.match(body.messages.at(-1).content, /Denied:/);
            controller.abort();
          }
          const items = scenario === 'multiple-active'
            ? [1, 2].map(id => ({ id, status: 'in_progress' }))
            : [{ id: 1, status: 'done', evidence: 'node --test positive input' },
              { id: 2, status: 'done', evidence: 'unobserved claim' }];
          return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [
            { id: 'test', type: 'function', function: { name: 'run_test', arguments: '{}' } },
            { id: 'batch', type: 'function', function: { name: 'update_checklist', arguments: JSON.stringify({ items }) } },
          ] } }] });
        },
      });
      assert.ok(result.error);
      assert.equal(requests, 2);
      assert.deepEqual(result.acceptanceContinuation.items.map(({ status }) => status), ['pending', 'pending']);
      assert.deepEqual(result.acceptanceContinuation.items.map(({ evidenceRef }) => evidenceRef), [null, null]);
    });
  }
});

test('acceptance continuation never reuses passing observations after changed bytes, failed tests or cancellation', async (context) => {
  for (const scenario of ['changed', 'failed', 'cancelled', 'test-mutated']) {
    await context.test(scenario, async () => {
      const controller = new AbortController();
      let bytes = 'before';
      let calls = 0;
      let testCalls = 0;
      let writes = 0;
      const task = planStub('Update `README.md` and `smoke.test.mjs`.',
        { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task;
      const config = { ...llmConfig, seat: { ...llmConfig.seat, evidence_workspace: true } };
      const tool = (id, name, args) => ({ id, type: 'function', function: { name, arguments: JSON.stringify(args) } });
      const result = await runLoop({ config, worktree: process.cwd(), context: { task, pack: task },
        signal: controller.signal,
        acceptanceSource: async () => acceptanceSourceIdentity(new Map([['README.md', bytes]])),
        verify: async () => ({ pass: true, reasons: [] }),
        tools: {
          run_test: async () => {
            testCalls += 1;
            if (scenario === 'test-mutated') bytes = `during-test-${testCalls}`;
            return { exit_code: scenario === 'failed' && testCalls > 1 ? 1 : 0, stdout: 'not ok failure', stderr: '' };
          },
          write_file: async ({ path: file }) => { writes += 1; bytes = 'after'; return { path: file }; },
        },
        fetchImpl: async () => {
          calls += 1;
          const message = calls === 1 ? { role: 'assistant', tool_calls: [
            tool('pass', 'run_test', {}),
            tool('complete', 'update_checklist', { items: [{ id: 1, status: 'done', evidence: 'node --test passed' }] }),
            ...(scenario === 'changed' ? [tool('change', 'write_file', { path: 'README.md', content: 'after' })] : []),
          ] } : scenario === 'cancelled' ? { role: 'assistant', tool_calls: [
            tool('forbidden-after-cancel', 'write_file', { path: 'README.md', content: 'after' }),
          ] } : { role: 'assistant', content: '1. done: earlier tests passed. All done.' };
          if (calls === 2 && scenario === 'cancelled') controller.abort();
          return Response.json({ model: 'local-model', usage: { prompt_tokens: 10, completion_tokens: 2 },
            choices: [{ finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message }] });
        },
      });
      assert.ok(result.error, `${scenario} must not return acceptance success`);
      assert.equal(result.acceptanceContinuation.items[0].status, 'pending');
      assert.ok(result.acceptanceContinuation.items.slice(1).every(({ status }) => status === 'pending'));
      assert.ok(result.acceptanceContinuation.observations.every(({ verdict, source }) => verdict !== 'pass' ||
        source === acceptanceSourceIdentity(new Map([['README.md', bytes]]))));
      if (scenario === 'cancelled') {
        assert.equal(result.acceptanceContinuation.outcome, 'cancelled');
        assert.equal(writes, 0);
        assert.equal(testCalls, 1);
      }
      if (scenario === 'failed') {
        assert.ok(testCalls > 1);
        assert.ok(result.acceptanceContinuation.observations.every(({ verdict }) => verdict !== 'pass'));
      }
      if (scenario === 'test-mutated') {
        assert.ok(result.acceptanceContinuation.observations.every(({ tool }) => tool !== 'run_test'));
      }
    });
  }
});
