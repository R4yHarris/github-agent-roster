import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { runBuiltinIssue } from '../src/lib/builtin.mjs';
import { buildComparisonSummary, evaluatePilotPair } from '../src/lib/eve-pilot.mjs';
import { isRunCancelled } from '../src/runtime/cancel.mjs';
import { passingReview } from './helpers/review.mjs';
import { git, llmConfig, multiFileFixture } from './helpers/builtin.mjs';

const checks = ['node --test exits 0', 'README has a Status section'];
const issueNumbers = { baseline: 42, eve: 43 };
const syntheticControls = (baseRevision, tools) => ({
  sampler: { mode: 'deterministic-test-double' },
  test_budget: { calls: 10, timeout_ms: 900_000 },
  tools,
  source_evidence: baseRevision,
});

async function runCondition(options, kind, baseRevision) {
  const number = issueNumbers[kind];
  options.issue.number = number;
  options.issue.url = `https://github.com/example/project/issues/${number}`;

  const seat = { ...llmConfig.seat };
  if (kind === 'eve') seat.evidence_workspace = true;
  else delete seat.evidence_workspace;
  const config = { ...llmConfig, seat };

  let recovering = false;
  let initial = true;
  let wrote = false;
  let reviews = 0;
  let testCalls = 0;
  let actualTestPasses = 0;
  const systems = [];
  const tools = [];
  const tool = (id, name, args) => ({ id, type: 'function',
    function: { name, arguments: JSON.stringify(args) } });

  const result = await runBuiltinIssue(number, {
    ...options,
    config,
    log: () => {},
    onRunEvent: (event) => {
      if (event.type === 'perspective-escalation') recovering = true;
    },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      const system = body.messages[0].content;
      let message;
      if (system.startsWith('You are the builtin planner seat.')) {
        message = { content: JSON.stringify({
          title: 'Add Status',
          acceptance_checks: checks,
          files_allowed: ['README.md', 'smoke.test.mjs'],
        }) };
      } else if (system.startsWith('You are the builtin research step.')) {
        message = { content: 'Inventory reviewed.' };
      } else if (system.startsWith('You are the builtin reviewer seat.')) {
        assert.equal(body.tools, undefined, 'the reviewer must receive no coder tools');
        reviews += 1;
        message = { content: passingReview(body) };
      } else {
        systems.push(system);
        const offeredTools = (body.tools ?? []).map((entry) => entry.function.name);
        if (!tools.length) tools.push(...offeredTools);
        else assert.deepEqual(offeredTools, tools, 'baseline and continuation tool controls stay fixed');
        if (recovering && !wrote) {
          wrote = true;
          message = { tool_calls: [
            tool('write', 'write_file', { path: 'README.md', content: '# Example\n\n## Status\nActive.\n' }),
            tool('test', 'run_test', {}),
            tool('checks', 'update_checklist', { items: checks.map((_, index) => ({
              id: index + 1,
              status: 'done',
              evidence: index === 0 ? 'node --test passed' : 'README.md contains the Status section',
            })) }),
          ] };
        } else if (initial) {
          initial = false;
          message = { tool_calls: [tool('working', 'update_checklist', {
            items: [{ id: 2, status: 'in_progress' }],
          })] };
        } else {
          message = { content: 'The requested Status section is ready for verification.' };
        }
      }
      return Response.json({ choices: [{
        finish_reason: message.tool_calls ? 'tool_calls' : 'stop',
        message: { role: 'assistant', ...message },
      }] });
    },
    runTestCommand: async (_program, args, options) => {
      testCalls += 1;
      assert.ok(testCalls <= 10, 'the deterministic test budget is bounded');
      if (!recovering) {
        throw Object.assign(new Error('deterministic handoff test failure'), {
          code: 1, stdout: 'not ok', stderr: 'injected only to force a fresh coder perspective',
        });
      }
      const execution = spawnSync(process.execPath, args, {
        cwd: options.cwd,
        encoding: 'utf8',
        timeout: 900_000,
        maxBuffer: 1_000_000,
      });
      if (execution.error) throw execution.error;
      if (execution.status !== 0) {
        throw Object.assign(new Error('fixture node --test failed'), {
          code: execution.status ?? 1, stdout: execution.stdout, stderr: execution.stderr,
        });
      }
      actualTestPasses += 1;
      return { stdout: execution.stdout, stderr: execution.stderr };
    },
  });

  assert.equal(result.result.excellence.pass, true);
  assert.equal(result.review.completed, true);
  assert.equal(result.review.verdict, 'pass');
  assert.equal(reviews, 1);
  assert.ok(actualTestPasses > 0, 'a final test result must come from the real fixture test command');
  assert.equal(git(options.target, 'rev-parse', 'HEAD'), baseRevision, 'task edits stay in the issue worktree');
  assert.ok(result.result.excellence.files.includes('README.md'));
  assert.ok(result.result.excellence.files.every((file) => ['README.md'].includes(file)));
  assert.ok(tools.includes('run_test'));
  assert.ok(tools.includes('write_file'));
  assert.ok(systems.length > 0);
  for (const run of Object.values(result.runs)) {
    if (run?.metrics) {
      assert.equal(run.metrics.prompt_tokens ?? null, null);
      assert.equal(run.metrics.completion_tokens ?? null, null);
    }
  }

  if (kind === 'baseline') {
    assert.equal(config.seat.evidence_workspace, undefined);
    assert.ok(systems.every((system) => !system.includes('## Acceptance continuation')));
    assert.equal(result.result.acceptanceContinuation, undefined);
  } else {
    assert.equal(config.seat.evidence_workspace, true);
    const handedOff = systems.filter((system) => system.includes('## Acceptance continuation'));
    assert.ok(handedOff.length > 0, 'the opted-in public lifecycle must carry a real handoff capsule');
    for (const system of handedOff) {
      assert.ok(system.includes(`1. [pending] ${checks[0]}`), system);
      assert.ok(system.includes(`2. [in_progress] ${checks[1]}`), system);
    }
    assert.equal(result.result.acceptanceContinuation.outcome, 'verified');
    assert.ok(result.result.acceptanceContinuation.items.every(({ status }) => status === 'done'));
  }

  return {
    kind,
    result,
    tools,
    testCalls,
    actualTestPasses,
    worktreePath: result.worktreePath,
  };
}

test('public baseline and opted-in lifecycle outcomes feed the paired evaluator without invented evidence', async (t) => {
  const options = multiFileFixture(t);
  const baseRevision = git(options.target, 'rev-parse', 'HEAD');
  const baseline = await runCondition(options, 'baseline', baseRevision);
  const eve = await runCondition(options, 'eve', baseRevision);
  assert.deepEqual(eve.tools, baseline.tools);

  const controls = syntheticControls(baseRevision, baseline.tools);
  const conditions = [
    { id: 'baseline-default-off', kind: 'baseline', behavior_bundle: { id: 'baseline-default-off' } },
    { id: 'evidence-workspace-opt-in', kind: 'eve', behavior_bundle: { id: 'evidence-workspace-opt-in' } },
  ].map((condition) => ({
    ...condition,
    model: 'deterministic-test-double',
    profile: 'offline-fixture',
    shared_controls: controls,
  }));
  const task = {
    id: 'synthetic-status-task',
    class: 'docs',
    difficulty: 1,
    kind: 'task',
    held_out: false,
    synthetic: true,
    base_revision: baseRevision,
    model: 'deterministic-test-double',
    profile: 'offline-fixture',
  };
  const records = [baseline, eve].map(({ kind, result }) => ({
    task_id: task.id,
    condition_id: conditions.find((condition) => condition.kind === kind).id,
    trial: 1,
    synthetic: true,
    context: {
      task_class: task.class,
      difficulty: task.difficulty,
      base_revision: baseRevision,
      model: task.model,
      profile: task.profile,
      behavior_bundle: conditions.find((condition) => condition.kind === kind).behavior_bundle.id,
      shared_controls: controls,
    },
    outcomes: {
      automated_tests: result.result.excellence.pass ? 'pass' : 'fail',
      review_verdict: result.review.verdict,
      human_acceptance: null,
    },
    metrics: null,
  }));

  const manifest = {
    schema: 1,
    synthetic: true,
    conditions,
    tasks: [task],
    thresholds: [{ metric: 'latency_ms', max_eve_median: 1000 }],
  };
  const summary = buildComparisonSummary(evaluatePilotPair(manifest, records));
  assert.deepEqual(buildComparisonSummary(evaluatePilotPair(manifest, records)), summary);

  assert.equal(summary.coverage.status, 'pilot-incomplete');
  assert.equal(summary.coverage.measured_held_out_tasks, 0);
  assert.equal(summary.paired_counts.human_acceptance.comparable_tasks, 0);
  assert.equal(summary.paired_counts.human_acceptance.unknown_pairs, 1);
  assert.equal(summary.thresholds[0].status, 'unknown');
  assert.equal(summary.thresholds[0].observed_eve_median, null);
  assert.equal(summary.tasks[0].synthetic, true);
  assert.equal(summary.tasks[0].held_out, false);
  assert.equal(summary.tasks[0].baseline.automated_tests, 'pass');
  assert.equal(summary.tasks[0].eve.automated_tests, 'pass');
  assert.equal(summary.tasks[0].baseline.review_verdict, 'pass');
  assert.equal(summary.tasks[0].eve.review_verdict, 'pass');
  assert.deepEqual([...new Set(summary.unknown_metrics.map(({ metric }) => metric))].sort(), [
    'completion_tokens', 'cost_cents', 'energy_joules', 'hardware', 'latency_ms', 'prompt_tokens',
  ]);
  assert.equal(baseline.actualTestPasses > 0, true);
  assert.equal(eve.actualTestPasses > 0, true);
});

for (const cancellationPoint of ['after-edit', 'after-green-tests']) {
  test(`paired public cancellation at ${cancellationPoint} preserves evidence without claiming delivery`, async (t) => {
    const options = multiFileFixture(t);
    const baseRevision = git(options.target, 'rev-parse', 'HEAD');
    const baseStatus = git(options.target, 'status', '--porcelain', '--untracked-files=no');
    const outcomes = [];
    for (const kind of ['baseline', 'eve']) {
      const controller = new AbortController();
      const number = issueNumbers[kind];
      options.issue.number = number;
      options.issue.url = `https://github.com/example/project/issues/${number}`;
      const seat = { ...llmConfig.seat };
      if (kind === 'eve') seat.evidence_workspace = true;
      else delete seat.evidence_workspace;
      let worktree;
      let coderCalls = 0;
      let actualTestPasses = 0;
      let publications = 0;
      let reviews = 0;
      let failure;
      let frozenTask;
      const offered = [];
      const tool = (id, name, args) => ({ id, type: 'function',
        function: { name, arguments: JSON.stringify(args) } });
      const changed = '# Example\n\n## Status\nActive.\n';
      try {
        await runBuiltinIssue(number, {
          ...options, config: { ...llmConfig, seat }, signal: controller.signal, log: () => {},
          onPrepared: prepared => { worktree = prepared.worktreePath; },
          publisher: async () => { publications++; assert.fail('Cancelled pilot must never publish'); },
          fetchImpl: async (_url, request) => {
            assert.equal(controller.signal.aborted, false, 'No inference after cancellation');
            const body = JSON.parse(request.body);
            const system = body.messages[0].content;
            let message;
            if (system.startsWith('You are the builtin planner seat.')) {
              message = { content: JSON.stringify({ title: 'Add Status',
                acceptance_checks: checks, files_allowed: ['README.md', 'smoke.test.mjs'] }) };
            } else if (system.startsWith('You are the builtin research step.')) {
              message = { content: 'Inventory reviewed.' };
            } else if (system.startsWith('You are the builtin reviewer seat.')) {
              reviews++;
              assert.fail('Cancelled coder must not reach reviewer');
            } else {
              coderCalls++;
              if (!offered.length) offered.push(...body.tools.map(entry => entry.function.name));
              if (coderCalls === 1) {
                message = { tool_calls: [
                  tool('write', 'write_file', { path: 'README.md', content: changed }),
                  ...(cancellationPoint === 'after-green-tests' ? [
                    tool('test', 'run_test', {}),
                    tool('checks', 'update_checklist', { items: checks.map((_, index) => ({
                      id: index + 1, status: 'done', evidence: 'node --test passed; README.md Status exists',
                    })) }),
                  ] : []),
                ] };
              } else {
                assert.equal(coderCalls, 2, 'Unexpected retry before cancellation');
                assert.equal(readFileSync(path.join(worktree, 'README.md'), 'utf8').replace(/\r\n/g, '\n'), changed);
                frozenTask = readFileSync(path.join(worktree, 'TASK.md'), 'utf8');
                controller.abort();
                // A late successful HTTP response still cannot authorize tools after revocation.
                message = { tool_calls: [
                  tool('late-write', 'write_file', { path: 'README.md', content: 'unauthorized late write' }),
                  tool('late-test', 'run_test', {}),
                ] };
              }
            }
            return Response.json({ choices: [{ finish_reason: message.tool_calls ? 'tool_calls' : 'stop',
              message: { role: 'assistant', ...message } }] });
          },
          runTestCommand: async (_program, args, invocation) => {
            assert.equal(controller.signal.aborted, false, 'No test process after cancellation');
            const env = { ...process.env };
            delete env.NODE_TEST_CONTEXT;
            const execution = spawnSync(process.execPath, args, { cwd: invocation.cwd,
              env, encoding: 'utf8', timeout: 30_000, maxBuffer: 1_000_000 });
            if (execution.error) throw execution.error;
            assert.equal(execution.status, 0, execution.stdout + execution.stderr);
            if (args[0] !== '--check') {
              assert.match(execution.stdout, /# pass 1\b/);
              actualTestPasses++;
            }
            return { stdout: execution.stdout, stderr: execution.stderr };
          },
        });
        assert.fail('Cancelled public lifecycle must reject, not return a completed run');
      } catch (error) {
        failure = error;
      }
      assert.ok(isRunCancelled(failure), failure?.stack);
      assert.equal(coderCalls, 2);
      assert.equal(reviews, 0);
      assert.equal(publications, 0);
      assert.equal(actualTestPasses, cancellationPoint === 'after-edit' ? 0 : 1);
      assert.equal(failure.result.excellence.pass, false);
      assert.equal(failure.result.tests?.exit_code ?? null, cancellationPoint === 'after-edit' ? null : 0);
      assert.equal(readFileSync(path.join(worktree, 'README.md'), 'utf8').replace(/\r\n/g, '\n'), changed);
      assert.match(readFileSync(path.join(worktree, 'RESULT.md'), 'utf8'), /Checks: FAIL[\s\S]*cancel/i);
      assert.equal(existsSync(path.join(worktree, 'REVIEW.md')), false);
      const task = readFileSync(path.join(worktree, 'TASK.md'), 'utf8');
      assert.equal(task, frozenTask, 'Cancellation and late tools cannot change the planned authority');
      assert.ok(task.includes('Add a Status section to README.md.'));
      for (const check of checks) assert.ok(task.includes(check));
      assert.equal(git(options.target, 'rev-parse', 'HEAD'), baseRevision);
      assert.equal(git(options.target, 'status', '--porcelain', '--untracked-files=no'), baseStatus);
      if (kind === 'eve') {
        const capsule = failure.result.acceptanceContinuation;
        assert.equal(capsule.outcome, 'cancelled');
        assert.ok(capsule.items.every(({ status, evidenceRef }) => status === 'pending' && evidenceRef === null));
        if (cancellationPoint === 'after-green-tests') assert.ok(capsule.notices.length > 0);
      } else {
        assert.equal(failure.result.acceptanceContinuation, undefined);
      }
      outcomes.push({ kind, tools: offered, tests: actualTestPasses ? 'pass' : null });
    }
    assert.deepEqual(outcomes[0].tools, outcomes[1].tools);
    const shared = syntheticControls(baseRevision, outcomes[0].tools);
    const conditions = outcomes.map(({ kind }) => ({
      id: kind, kind, model: 'deterministic-test-double', profile: 'offline-fixture',
      behavior_bundle: { id: kind }, shared_controls: shared,
    }));
    const task = { id: cancellationPoint, class: 'test', difficulty: 2, kind: 'fault',
      held_out: false, synthetic: true, base_revision: baseRevision,
      model: 'deterministic-test-double', profile: 'offline-fixture' };
    const records = outcomes.map(({ kind, tests }) => ({
      task_id: task.id, condition_id: kind, trial: 1, synthetic: true,
      context: { task_class: task.class, difficulty: task.difficulty, base_revision: baseRevision,
        model: task.model, profile: task.profile, behavior_bundle: kind, shared_controls: shared },
      outcomes: { automated_tests: tests, review_verdict: null, human_acceptance: null }, metrics: null,
      safety_violations: [], fault_violations: [],
    }));
    const manifest = { schema: 1, synthetic: true, conditions, tasks: [task],
      thresholds: [{ metric: 'latency_ms', max_eve_median: 1000 }] };
    const summary = buildComparisonSummary(evaluatePilotPair(manifest, records));
    assert.equal(summary.coverage.status, 'pilot-incomplete');
    assert.equal(summary.coverage.measured_held_out_tasks, 0);
    assert.equal(summary.coverage.adversarial_measured_held_out, 0);
    assert.equal(summary.fault_cases[0].baseline.automated_tests, outcomes[0].tests);
    assert.equal(summary.fault_cases[0].eve.automated_tests, outcomes[1].tests);
    for (const condition of ['baseline', 'eve']) {
      assert.equal(summary.fault_cases[0][condition].review_verdict, null);
      assert.equal(summary.fault_cases[0][condition].human_acceptance, null);
    }
    assert.equal(summary.thresholds[0].status, 'unknown');
    assert.equal(summary.paired_counts.human_acceptance.comparable_tasks, 0);
    assert.equal(summary.paired_counts.human_acceptance.unknown_pairs, 0, 'Fault cases are not task pairs');
    assert.deepEqual([...new Set(summary.unknown_metrics.map(({ metric }) => metric))].sort(), [
      'completion_tokens', 'cost_cents', 'energy_joules', 'hardware', 'latency_ms', 'prompt_tokens',
    ]);
    assert.equal(summary.safety_gate, 'no-reported-violations');
  });
}
