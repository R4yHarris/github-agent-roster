import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { runBuiltinIssue } from '../src/lib/builtin.mjs';
import { isRunCancelled } from '../src/runtime/cancel.mjs';
import { multiFileFixture, llmConfig, git } from './helpers/builtin.mjs';

const recording = JSON.parse(readFileSync(new URL('./fixtures/lifecycle-replay.json', import.meta.url), 'utf8'));

for (const scenario of recording.scenarios) {
  test(`lifecycle replay: ${scenario.name}`, async (context) => {
    const options = multiFileFixture(context);
    const baseHead = git(options.target, 'rev-parse', 'HEAD');
    const baseDiff = git(options.target, 'diff', 'HEAD');
    const baseStatus = git(options.target, 'status', '--porcelain', '--untracked-files=no');
    const controller = new AbortController();
    let requests = 0;
    let tools = 0;
    let tests = 0;
    let result;
    let failure;
    let replayFailure;
    let publications = 0;
    const toolResults = [];
    const reviewRepairs = [];
    const logs = [];
    const pending = runBuiltinIssue(42, {
      ...options, config: llmConfig, signal: controller.signal, log: (text) => logs.push(text),
      publisher: async () => {
        publications += 1;
        assert.fail('Lifecycle replays must never publish');
      },
      onRunEvent: (event) => {
        if (event.type === 'tool-result') toolResults.push({ name: event.name, status: event.status });
        if (event.type === 'review-repair') reviewRepairs.push({ attempt: event.attempt, unmetChecks: event.unmetChecks });
      },
      fetchImpl: async (_url, request) => {
        try {
          assert.equal(controller.signal.aborted, false, 'No inference after cancellation');
          const body = JSON.parse(request.body);
          const system = body.messages[0].content;
          assert.doesNotMatch(system, /## Acceptance continuation/, 'default-disabled replays keep their original context');
          const role = system.match(/^You are the builtin (planner|reviewer) seat\./)?.[1]
            ?? (body.messages[1]?.content?.startsWith('Complete this task using only the offered tools.') ? 'coder' : undefined);
          const step = scenario.steps[requests++];
          assert.ok(step, `Unexpected HTTP attempt ${requests}: ${role}`);
          assert.equal(role, step[0], `HTTP attempt ${requests} changed seat order`);
          if (role === 'reviewer') assert.equal(body.tools, undefined);
          if (step[1] === 'cancel') {
            assert.equal(readFileSync(path.join(options.target, '.worktrees', 'issue-42', 'README.md'), 'utf8')
              .replace(/\r\n/g, '\n'), scenario.expected.readme ?? '# Example\n',
            'Recorded edits must already be persisted when cancellation occurs');
            controller.abort();
            request.signal.throwIfAborted();
          }
          const message = structuredClone(recording.responses[step[1]]);
          assert.ok(message, `Missing recorded response ${step[1]}`);
          if (message.content === '$plan') message.content = JSON.stringify(recording.plan);
          tools += message.tool_calls?.length ?? 0;
          return Response.json({ choices: [{
            finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message,
          }] });
        } catch (error) {
          if (!request.signal.aborted || error !== request.signal.reason) replayFailure = error;
          throw error;
        }
      },
      runTestCommand: async (_program, args) => {
        assert.equal(controller.signal.aborted, false, 'No tests after cancellation');
        if (args[0] === '--check') return { stdout: '', stderr: '' };
        const outcome = scenario.tests[tests++];
        assert.ok(outcome, `Unexpected test invocation ${tests}`);
        if (outcome === 'fail') throw Object.assign(new Error('tests failed'), {
          code: 1, stdout: 'not ok', stderr: 'fixture assertion failed',
        });
        return { stdout: 'pass', stderr: '' };
      },
    });
    try { result = await pending; } catch (error) { failure = error; }
    await new Promise((resolve) => setImmediate(resolve));
    assert.ifError(replayFailure);
    assert.equal(requests, scenario.expected.requests, failure?.stack);
    assert.equal(requests, scenario.steps.length, 'Every recorded response must be consumed');
    assert.equal(tools, scenario.expected.tools);
    assert.deepEqual(toolResults.filter(({ name }) => name === 'write_file'),
      Array.from({ length: 3 + scenario.expected.tools }, () => ({ name: 'write_file', status: 'ok' })));
    assert.deepEqual(toolResults.filter(({ name }) => name !== 'write_file'),
      Array.from({ length: scenario.expected.tests }, () => ({ name: 'run_test', status: 'ok' })));
    assert.equal(tests, scenario.expected.tests);
    assert.equal(tests, scenario.tests.length, 'Every recorded test outcome must be consumed');
    assert.equal(publications, 0);
    assert.deepEqual(reviewRepairs, Array.from({ length: scenario.expected.reviewRepairs ?? 0 },
      (_, index) => ({ attempt: index + 1, unmetChecks: [2] })));
    const worktree = path.join(options.target, '.worktrees', 'issue-42');
    const task = readFileSync(path.join(worktree, 'TASK.md'), 'utf8');
    assert.match(task, /## Acceptance checks/);
    for (const check of recording.plan.acceptance_checks) assert.ok(task.includes(check));
    assert.match(task, /Files allowed[\s\S]*`README\.md`/);
    assert.ok(existsSync(path.join(worktree, 'RECIPE.yml')));
    assert.ok(existsSync(path.join(worktree, 'ESTIMATE.md')));
    assert.equal(readFileSync(path.join(worktree, 'README.md'), 'utf8').replace(/\r\n/g, '\n'),
      scenario.expected.readme ?? (scenario.expected.error ? '# Example\n' : '# Example\n\n## Status\nActive.\n'));
    if (scenario.expected.error) {
      assert.ok(failure, 'Failed replay cannot return success');
      assert.match(failure.message, new RegExp(scenario.expected.error, 'i'));
      assert.equal(result, undefined, 'Stopped replays cannot return a completed delivery');
      assert.equal(isRunCancelled(failure), controller.signal.aborted);
      assert.equal(failure.result.excellence.pass, false);
      if (controller.signal.aborted) {
        assert.equal(failure.result.testRepairs ?? 0, 0);
        assert.match(readFileSync(path.join(worktree, 'RESULT.md'), 'utf8'), /cancel/i);
      }
      assert.equal(existsSync(path.join(worktree, 'REVIEW.md')), false);
      if (scenario.name === 'all-fail') {
        assert.equal(failure.result.repairRepeated, true);
        assert.match(readFileSync(path.join(worktree, 'RESULT.md'), 'utf8'), /test/i);
      }
    } else {
      assert.ifError(failure);
      assert.equal(result.result.testRepairs, scenario.expected.repairs);
      assert.equal(result.result.excellence.pass, true);
      assert.equal(result.review.verdict, scenario.expected.verdict);
      assert.equal(result.reviewRepairs?.length ?? 0, scenario.expected.reviewRepairs ?? 0);
      assert.equal(result.result.usage?.prompt_tokens, undefined);
      assert.equal(result.result.usage?.completion_tokens, undefined);
      assert.equal(result.review.usage?.prompt_tokens, undefined);
      assert.equal(result.review.usage?.completion_tokens, undefined);
      for (const seat of ['planner', 'coder', 'reviewer']) {
        assert.ok(result.runs[seat], `${seat} must retain response-backed model evidence`);
        assert.equal(result.runs[seat].metrics.context_used, undefined);
        assert.equal(result.runs[seat].metrics.context_out, undefined);
        assert.equal(result.runs[seat].metrics.model, 'local-model');
      }
      assert.match(readFileSync(result.result.resultPath, 'utf8'), /Added Status/);
      assert.match(readFileSync(result.review.reviewPath, 'utf8'), new RegExp(`Verdict: ${scenario.expected.verdict}`));
      if (scenario.expected.verdict === 'fail') {
        assert.equal(isRunCancelled(failure), false);
        assert.equal(result.review.completed, true);
        assert.deepEqual(result.review.unmetChecks, [2]);
        assert.deepEqual(result.reviewRepairs.map(({ unmetChecks }) => unmetChecks), [[2], [2]]);
        assert.equal(result.command, null, 'Exhausted review repairs must keep the publication gate closed');
        assert.match(logs.join('\n'), /Publication unavailable: REVIEW\.md failed/);
        assert.match(readFileSync(result.review.reviewPath, 'utf8'), /Status needs more detail/);
      }
    }
    assert.equal(readFileSync(path.join(options.target, 'README.md'), 'utf8').replace(/\r\n/g, '\n'), '# Example\n');
    assert.equal(git(options.target, 'rev-parse', 'HEAD'), baseHead);
    assert.equal(git(options.target, 'diff', 'HEAD'), baseDiff);
    assert.equal(git(options.target, 'status', '--porcelain', '--untracked-files=no'), baseStatus);
    assert.equal(options.calls.some(({ program, args }) =>
      program === 'gh' && args[0] !== 'issue' || program === 'git' && args[0] === 'push'), false);
  });
}
