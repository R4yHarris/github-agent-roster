import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';
import { createProvenanceStore } from '../src/lib/provenance-api.mjs';
import { filterRecords } from '../src/lib/history-query.mjs';
import { runBuiltinAsk } from '../src/lib/builtin.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { fixture, runBuiltinIssue, stubConfig, llmConfig } from './helpers/builtin.mjs';

async function records(options) {
  const persisted = await createProvenanceStore({
    root: path.join(options.base, 'machine', 'provenance'), repoRoot: options.target,
  }).query();
  for (const record of persisted) {
    assert.deepEqual(record.issue, { issue: '42', task: 'issue-42' });
    assert.deepEqual(record.seat, { name: record.event === 'session'
      ? record.sessionId.split('-').at(-1) : '' });
  }
  assert.equal(filterRecords(persisted, { issue: 42 }).length, persisted.length);
  return persisted
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
  assert.equal(filterRecords(persisted, { issue: '42', seat: 'coder' }).length, 1);
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

test('local asks retain canonical task and real seat identity without inventing an issue number', async (t) => {
  const options = fixture(t);
  const result = await runBuiltinAsk('Add a one-line Status section to README.md.', {
    ...options, config: stubConfig, log: () => {},
  });
  const root = path.join(options.base, 'machine', 'provenance');
  const persisted = await createProvenanceStore({ root, repoRoot: options.target }).query();
  assert.ok(persisted.length > 0);
  for (const record of persisted) {
    assert.deepEqual(record.issue, { issue: '', task: result.task });
    assert.equal(record.payload.task, result.task);
  }
  const sessions = persisted.filter((record) => record.event === 'session');
  assert.deepEqual(sessions.map((record) => record.seat.name).sort(), ['coder', 'planner', 'reviewer']);
  assert.equal(filterRecords(persisted, { issue: 42 }).length, 0);
  assert.equal(filterRecords(persisted, { seat: 'coder' }).length, 1);
  assert.deepEqual((await openProvenanceStore(root).readAll()).records.map((record) => record.id).sort(),
    persisted.map((record) => record.id).sort());
});

test('planner critic and revision declare their seat without borrowed timing or tool evidence', async (t) => {
  const options = fixture(t);
  mkdirSync(path.join(options.target, '.roster'), { recursive: true });
  writeFileSync(path.join(options.target, '.roster', 'fleet.yml'), formatFleet({ profiles: [{
    id: 'critic-fixture', base_url: 'http://fixture.invalid/v1', model: 'critic-model', provider: 'vllm',
    context_max: 32768, concurrency: 1, hardware: 'fixture-only', notes: '',
  }] }));
  let criticCalls = 0;
  await runBuiltinIssue(42, { ...options,
    config: { ...llmConfig, planner: { ...llmConfig.planner, critic_profile: 'critic-fixture' } },
    confirm: true, log: () => {}, vault: { get: async () => undefined },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      return Response.json({ model: body.model, choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: body.model === 'critic-model'
          ? JSON.stringify({ defects: ++criticCalls === 1
            ? [{ check: 1, problem: 'Clarify the requested Status evidence.',
              fix: 'Name the Status section in the acceptance check.' }] : [] }) :
          JSON.stringify({ title: 'Add Status', acceptance_checks: ['README has a Status section'],
            files_allowed: ['README.md'] }),
      } }] });
    },
  });
  const persisted = await createProvenanceStore({
    root: path.join(options.base, 'machine', 'provenance'), repoRoot: options.target,
  }).query({ event: 'session' });
  const critic = persisted.find((record) => record.sessionId === 'roster-42-planner-critic-1');
  assert.ok(critic);
  assert.deepEqual(critic.seat, { name: 'planner' });
  assert.deepEqual(critic.issue, { issue: '42', task: 'issue-42' });
  assert.equal(critic.servedModel, 'critic-model');
  assert.equal(critic.startedAt, null);
  assert.equal(critic.endedAt, null);
  assert.equal(critic.evidence.observed_tool_events, undefined);
  const revision = persisted.find((record) => record.sessionId === 'roster-42-planner-revision');
  assert.ok(revision);
  assert.deepEqual(revision.seat, { name: 'planner' });
  assert.deepEqual(revision.issue, { issue: '42', task: 'issue-42' });
  assert.equal(revision.startedAt, null);
  assert.equal(revision.endedAt, null);
  assert.equal(revision.evidence.observed_tool_events, undefined);
});
