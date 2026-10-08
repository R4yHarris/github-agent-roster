import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';
import { fixture, runBuiltinIssue, stubConfig, llmConfig } from './helpers/builtin.mjs';

async function records(options) {
  return (await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll()).records
    .sort((left, right) => left.seq - right.seq);
}

test('builtin lifecycle persists started, seats and completed under one run id', async (t) => {
  const options = fixture(t);
  await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  const persisted = await records(options);
  assert.equal(persisted[0].event, 'started');
  assert.equal(persisted.at(-1).event, 'completed');
  assert.equal(persisted.at(-1).payload.outcome, 'unverified');
  assert.deepEqual(persisted.filter((record) => record.event === 'session').map((record) => record.sessionId),
    ['roster-42-planner', 'roster-42-coder', 'roster-42-reviewer']);
  assert.equal(new Set(persisted.map((record) => record.runId)).size, 1);
  assert.ok(persisted.every((record) => record.repoIdentity));
});

test('failure before planner starts records failure and preserves the original error', async (t) => {
  const options = fixture(t);
  const failure = new Error('prepared handoff failed');
  await assert.rejects(runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    onPrepared: () => { throw failure; },
  }), (error) => error === failure);
  const persisted = await records(options);
  assert.deepEqual(persisted.map((record) => record.event), ['started', 'failure']);
});

test('explicit cancellation before planning records cancellation, never completion', async (t) => {
  const options = fixture(t);
  const controller = new AbortController();
  await assert.rejects(runBuiltinIssue(42, { ...options, config: llmConfig, log: () => {},
    signal: controller.signal, onPrepared: () => controller.abort(),
    fetchImpl: () => assert.fail('cancelled run must not contact the fleet'),
  }), { code: 'ROSTER_CANCELLED' });
  assert.deepEqual((await records(options)).map((record) => record.event), ['started', 'cancellation']);
});

test('unverified planner failure is recorded as failure, not completed', async (t) => {
  const options = fixture(t);
  let calls = 0;
  const result = await runBuiltinIssue(42, { ...options, config: llmConfig, log: () => {},
    fetchImpl: async () => Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
      role: 'assistant', tool_calls: [{ id: `bad-${++calls}`, type: 'function',
        function: { name: 'write_file', arguments: 'garbage' } }],
    } }] }),
    runTestCommand: () => assert.fail('planning failure cannot run tests'),
  });
  assert.equal(result.failed, true);
  assert.equal(calls, 2);
  const persisted = await records(options);
  assert.equal(persisted[0].event, 'started');
  assert.equal(persisted.at(-1).event, 'failure');
  assert.ok(!persisted.some((record) => record.event === 'completed'));
});

test('durable history opt-out preserves execution without creating records', async (t) => {
  const options = fixture(t);
  const logs = [];
  await runBuiltinIssue(42, { ...options, config: stubConfig,
    env: { ...options.env, ROSTER_PROVENANCE_OPT_OUT: 'true' }, log: (line) => logs.push(line),
  });
  assert.deepEqual(await records(options), []);
  assert.ok(!logs.some((line) => line.includes('durable history is incomplete')));
});
