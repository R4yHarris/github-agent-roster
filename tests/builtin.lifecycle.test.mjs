import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';
import { createProvenanceStore } from '../src/lib/provenance-api.mjs';
import { filterRecords } from '../src/lib/history-query.mjs';
import { runBuiltinAsk } from '../src/lib/builtin.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { fixture, git, runBuiltinIssue, stubConfig, llmConfig } from './helpers/builtin.mjs';

async function records(options, commit = git(options.target, 'rev-parse', 'HEAD')) {
  const persisted = await createProvenanceStore({
    root: path.join(options.base, 'machine', 'provenance'), repoRoot: options.target,
  }).query();
  for (const record of persisted) {
    assert.deepEqual(record.repository, { remote: '', commit });
    assert.deepEqual(record.issue, { issue: '42', task: 'issue-42' });
    assert.deepEqual(record.seat, { name: record.event === 'session'
      ? record.sessionId.split('-').at(-1) : '' });
  }
  assert.equal(JSON.stringify(persisted).includes('https://github.com/example/project.git'), false);
  assert.equal(JSON.stringify(persisted).includes(JSON.stringify(options.target).slice(1, -1)), false);
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
    runCommand: (program, args, cwd) => {
      assert.notDeepEqual(args, ['rev-parse', '--verify', 'HEAD'], 'opt-out must not read provenance HEAD');
      return options.runCommand(program, args, cwd);
    },
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
    assert.deepEqual(record.repository, { remote: '', commit: git(options.target, 'rev-parse', 'HEAD') });
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

test('starting revision is read once from the worktree and remains fixed after HEAD changes', async (t) => {
  const options = fixture(t);
  const initial = git(options.target, 'rev-parse', 'HEAD');
  const reads = [];
  let later;
  await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    runCommand: (program, args, cwd) => {
      if (program === 'git' && args.join(' ') === 'rev-parse --verify HEAD') reads.push(cwd);
      return options.runCommand(program, args, cwd);
    },
    onPrepared: (prepared) => {
      git(prepared.worktreePath, '-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
        'commit', '--allow-empty', '-m', 'Advance worktree after provenance preparation');
      later = git(prepared.worktreePath, 'rev-parse', 'HEAD');
    },
  });
  assert.notEqual(later, initial);
  assert.deepEqual(reads, [path.join(options.target, '.worktrees', 'issue-42')]);
  assert.ok((await records(options, initial)).length > 0);
});

test('reused worktree provenance observes its actual HEAD rather than the caller revision', async (t) => {
  const options = fixture(t);
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  git(options.target, 'worktree', 'add', '-b', 'issue-42', worktree);
  git(worktree, '-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '--allow-empty', '-m', 'Existing task revision');
  const expected = git(worktree, 'rev-parse', 'HEAD');
  assert.notEqual(expected, git(options.target, 'rev-parse', 'HEAD'));
  await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  assert.ok((await records(options, expected)).length > 0);
});

test('unavailable revision stays empty, reports safely and does not mask a run failure', async (t) => {
  for (const mode of ['read-error', 'invalid-output']) {
    await t.test(mode, async (context) => {
      const options = fixture(context);
      const marker = 'test-only-private-api-key';
      const logs = [];
      const original = new Error('original handoff failure');
      let reads = 0;
      await assert.rejects(runBuiltinIssue(42, { ...options, config: stubConfig, log: (line) => logs.push(line),
        runCommand: (program, args, cwd) => {
          if (program === 'git' && args.join(' ') === 'rev-parse --verify HEAD') {
            reads++;
            if (mode === 'read-error') throw new Error(marker);
            return marker;
          }
          return options.runCommand(program, args, cwd);
        },
        onPrepared: () => { throw original; },
      }), (error) => error === original);
      assert.equal(reads, 1);
      assert.equal(logs.filter((line) => line.startsWith('Provenance starting commit')).length, 1);
      assert.equal(logs.join('\n').includes(marker), false);
      const persisted = await records(options, '');
      assert.deepEqual(persisted.map((record) => record.event), ['started', 'failure']);
      assert.equal(JSON.stringify(persisted).includes(marker), false);
    });
  }
});

test('a full SHA-256 revision is retained but partial and malformed object IDs remain unknown', async (t) => {
  for (const [output, expected] of [
    [` ${'A'.repeat(64)}\n`, 'a'.repeat(64)],
    ['a'.repeat(41), ''],
    ['a'.repeat(63), ''],
    [`${'a'.repeat(40)}\nextra`, ''],
  ]) {
    await t.test(`${output.length} characters`, async (context) => {
      const options = fixture(context);
      const logs = [];
      await runBuiltinIssue(42, { ...options, config: stubConfig, log: (line) => logs.push(line),
        runCommand: (program, args, cwd) => program === 'git' && args.join(' ') === 'rev-parse --verify HEAD'
          ? output : options.runCommand(program, args, cwd),
      });
      assert.ok((await records(options, expected)).some((record) => record.event === 'completed'));
      assert.equal(logs.filter((line) => line.startsWith('Provenance starting commit')).length, expected ? 0 : 1);
    });
  }
});

test('local ask captures a real starting revision through the default bounded Git reader', async (t) => {
  const options = fixture(t);
  const result = await runBuiltinAsk('Add a one-line Status section to README.md.', {
    ...options, runCommand: undefined, start: { base: 'current', sync: 'offline' },
    config: stubConfig, log: () => {},
  });
  const persisted = await createProvenanceStore({
    root: path.join(options.base, 'machine', 'provenance'), repoRoot: options.target,
  }).query();
  assert.ok(persisted.some((record) => record.event === 'completed'));
  assert.ok(persisted.every((record) => record.repository.commit === git(result.worktreePath, 'rev-parse', 'HEAD')));
});

test('an unreadable starting revision does not block otherwise healthy execution', async (t) => {
  const options = fixture(t);
  const logs = [];
  await runBuiltinIssue(42, { ...options, config: stubConfig, log: (line) => logs.push(line),
    runCommand: (program, args, cwd) => {
      if (program === 'git' && args.join(' ') === 'rev-parse --verify HEAD') {
        throw new Error('test-only-private-api-key');
      }
      return options.runCommand(program, args, cwd);
    },
  });
  assert.ok((await records(options, '')).some((record) => record.event === 'completed'));
  assert.equal(logs.filter((line) => line.startsWith('Provenance starting commit')).length, 1);
  assert.equal(logs.join('\n').includes('test-only-private-api-key'), false);
});
