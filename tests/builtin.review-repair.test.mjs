// Builtin seat orchestration: Docs review repair, repaired tests, confirm pauses, and direct asks.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { passingReview, reviewedChecks } from './helpers/review.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { writeAsk } from '../src/lib/ask.mjs';
import {
  coderStuckReason, maxPerspectiveEscalations, perspectiveContinuation, maxRescopes, rescopeBudget, rescopeContinuation,
  maxReviewRepairs, previousReviewContinuation, reviewRepairContinuation,
  prepareBuiltinPublication, runBuiltinAsk, runBuiltinIssue as runIssueWithSeats, stageReviewedFiles,
} from '../src/lib/builtin.mjs';
import { ToolAccessError } from '../src/runtime/tools.mjs';
import { loadRouteQuarantine, recordRouteQuarantine, routeQuarantineTtlMs } from '../src/lib/route-quarantine.mjs';
import { loadLearning } from '../src/lib/learn.mjs';
import { readStatus, formatStatus } from '../src/lib/status.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';
import { buildPublishMessage } from '../src/lib/publication.mjs';
import { parseRecipe } from '../src/lib/recipe.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { renderIssueBody } from '../src/lib/issue.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { LlmTimeoutError } from '../src/llm/request.mjs';
import { packAgentRun } from '../vendor/github-agent-contracts/scripts/parse-agent-run.mjs';
import { recordedCoderRun } from '../src/lib/seat-publication.mjs';
import { createDebugLog } from '../src/lib/debug-log.mjs';
import { readLocalRun } from '../src/lib/local-runs.mjs';
import { createSteeringControl } from '../src/runtime/steering.mjs';
import {
  runBuiltinIssue, example, stubConfig, llmConfig, vllmConfig, multiFileScope, git, fixture, multiFileFixture,
} from './helpers/builtin.mjs';

test('a bounded docs review failure returns its findings to the coder, which repairs the result', async (context) => {
  const options = fixture(context);
  options.issue.body = renderIssueBody(options.issue.body, {
    task_class: 'docs', difficulty: 1, estimate_min: 10,
  });
  let coderCalls = 0;
  let reviews = 0;
  let tests = 0;
  const result = await runIssueWithSeats(42, {
    ...options,
    config: llmConfig,
    log: () => {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      const system = body.messages[0].content;
      if (system.startsWith('You are the builtin planner seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({
            title: 'Add Status',
            acceptance_checks: ['node --test exits 0', 'README has a Status section'],
            files_allowed: ['README.md'],
          }),
        } }] });
      }
      if (system.startsWith('You are the builtin reviewer seat.')) {
        reviews += 1;
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: reviews === 1 ? JSON.stringify({
            verdict: 'fail', reasons: ['Status must say Active.'], security_notes: [],
          }) : passingReview(body),
        } }] });
      }
      coderCalls += 1;
      if (reviews === 1) assert.match(JSON.stringify(body.messages), /Review repair 1[\s\S]*Status must say Active/);
      const draft = coderCalls <= 2 ? 'Draft.' : 'Active.';
      const tools = body.tools ?? [];
      return Response.json({ choices: [tools.length ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: `save-${coderCalls}`, type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({
            path: 'README.md', content: `# Example\n\n## Status\n${draft}\n`,
          }),
        } }],
      } } : { finish_reason: 'stop', message: {
        role: 'assistant', content: `Saved ${draft} and passed checks.`,
      } }] });
    },
    runTestCommand: async () => {
      tests += 1;
      return { stdout: 'pass', stderr: '' };
    },
  });
  assert.equal(result.review.verdict, 'pass');
  assert.equal(reviews, 2);
  assert.equal(coderCalls, 4);
  assert.deepEqual(result.reviewRepairs.map(({ unmetChecks }) => unmetChecks), [[]]);
  assert.match(readFileSync(path.join(result.worktreePath, 'README.md'), 'utf8'), /## Status\nActive\./);
});

test('a repaired failing test passes excellence, read-only review, and declared-scope publication staging', async (context) => {
    const options = multiFileFixture(context);
    let tests = 0;
    let coderTurns = 0;
    const result = await runBuiltinIssue(42, { ...options, config: llmConfig, log: () => {},
      fetchImpl: async (_url, request) => {
        const body = JSON.parse(request.body);
        const system = body.messages[0].content;
        if (system.startsWith('You are the builtin planner seat.')) {
          return Response.json({ choices: [{ finish_reason: 'stop', message: {
            role: 'assistant', content: JSON.stringify({ title: 'Add Status',
              acceptance_checks: ['node --test exits 0'], files_allowed: multiFileScope }),
          } }] });
        }
        if (system.startsWith('You are the builtin reviewer seat.')) {
          assert.equal(tests, 2);
          assert.match(body.messages[1].content, /smoke\.test\.mjs/);
          return Response.json({ choices: [{ finish_reason: 'stop', message: {
            role: 'assistant', content: passingReview(body, {
              reasons: ['Tests passed after the repair.'], security_notes: ['No protected paths changed.'] }),
          } }] });
        }
        coderTurns += 1;
        const call = coderTurns === 1 ? { name: 'write_file', args: { path: 'README.md',
          content: '# Example\n\n## Status\nReady.\n' } }
          : coderTurns === 3 ? { name: 'read_file', args: { path: 'smoke.test.mjs' } }
            : coderTurns === 4 ? { name: 'write_file', args: { path: 'smoke.test.mjs',
              content: "import test from 'node:test';\ntest('smoke', () => {});\n" } } : null;
        if (coderTurns === 3) assert.match(body.messages.at(-1).content, /Repair 1 of 4[\s\S]*smoke\.test\.mjs/);
        return Response.json({ choices: [{ finish_reason: call ? 'tool_calls' : 'stop', message: call ? {
          role: 'assistant', tool_calls: [{ id: `code-${coderTurns}`, type: 'function', function: {
            name: call.name, arguments: JSON.stringify(call.args),
          } }],
        } : { role: 'assistant', content: 'Added Status.' } }] });
      },
      runTestCommand: async () => {
        tests += 1;
        if (tests === 1) throw Object.assign(new Error('test failed'), {
          code: 1, stdout: 'test at smoke.test.mjs:3:1', stderr: 'assertion failed',
        });
        return { stdout: 'pass', stderr: '' };
      },
    });
    assert.equal(result.result.testRepairs, 1);
    assert.deepEqual(result.result.repairFiles, ['smoke.test.mjs']);
    assert.equal(result.result.excellence.pass, true);
    assert.equal(result.review.verdict, 'pass');
    assert.match(result.planner.task, /Files allowed\n[\s\S]*- `smoke\.test\.mjs`/);
    const prepared = await prepareBuiltinPublication(result, { cwd: options.cwd, config: llmConfig,
      env: { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'test-only-key.pem' } });
    assert.equal(git(prepared.worktreePath, 'diff', '--cached', '--name-only'), 'README.md\nsmoke.test.mjs');
});

test('--confirm is the explicit slice pause, including declared file scope', async (context) => {
  const options = fixture(context);
  const logs = [];
  const first = await runBuiltinIssue(42, { ...options, config: stubConfig, confirm: true,
    log: (message) => logs.push(message) });
  assert.equal(first.confirmedPause, true);
  assert.equal(first.runs.coder, null);
  assert.equal(existsSync(path.join(first.worktreePath, 'RESULT.md')), false);
  assert.equal(existsSync(path.join(first.worktreePath, 'REVIEW.md')), false);
  assert.match(logs.join('\n'), /Task summary:[\s\S]*Paused by --confirm/);
  const next = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  assert.equal(next.planner.reused, true);
  assert.equal(next.result.mode, 'stub');
  await assert.rejects(runBuiltinIssue(42, { ...options, config: stubConfig, confirm: true, publish: true }),
    /--confirm cannot be combined with --publish/);
});

test('direct local asks run to RESULT and REVIEW in a Git worktree without gh', async (context) => {
  const options = fixture(context);
  const logs = [];
  const result = await runBuiltinAsk('Add a one-line Status section to README.md.', {
    ...options, config: stubConfig, log: (message) => logs.push(message),
    runCommand: async (program, args, workingDirectory) => {
      assert.equal(program, 'git', 'A local ask must never invoke gh');
      return options.runCommand(program, args, workingDirectory);
    },
  });
  assert.equal(result.local, true);
  assert.equal(result.issue.number, undefined);
  assert.equal(result.planningOnly, undefined);
  assert.equal(result.worktreePath, path.join(options.target, '.worktrees', result.task));
  assert.equal(git(result.worktreePath, 'branch', '--show-current'), result.task);
  assert.equal(parseRecipe(result.planner.recipe).ask, `local:${result.task}`);
  assert.equal(existsSync(result.result.resultPath), true);
  assert.equal(existsSync(result.review.reviewPath), true);
  assert.match(logs.join('\n'), /Task summary:[\s\S]*Allowed files: README\.md/);
  assert.match(readFileSync(result.assignmentPath, 'utf8'), /# Local Ask/);
});

test('a direct initiative prints PLAN and does not run coder or reviewer', async (context) => {
  const options = fixture(context);
  const logs = [];
  const result = await runBuiltinAsk('Build an orchestrator.', {
    ...options, config: stubConfig, log: (message) => logs.push(message),
    runCommand: async (program, args, workingDirectory) => {
      assert.equal(program, 'git');
      return options.runCommand(program, args, workingDirectory);
    },
  });
  assert.equal(result.askKind, 'initiative');
  assert.equal(result.planningOnly, true);
  assert.equal(existsSync(result.planPath), true);
  assert.equal(existsSync(path.join(result.worktreePath, 'RESULT.md')), false);
  assert.equal(existsSync(path.join(result.worktreePath, 'REVIEW.md')), false);
  assert.match(logs.join('\n'), /PLAN:/);
  assert.doesNotMatch(logs.join('\n'), /Task summary:/);
});

test('a configured direct ask implements and reviews the slice after its streamed summary', async (context) => {
  const options = fixture(context);
  const logs = [];
  let coderTurns = 0;
  let tests = 0;
  const result = await runBuiltinAsk('Add a one-line Status section to README.md.', {
    ...options, config: llmConfig, log: (message) => logs.push(message),
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      const system = body.messages[0].content;
      if (system.startsWith('You are the builtin planner seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({
            title: 'Add Status to README',
            acceptance_checks: ['The requested behavior in the Ask is implemented'],
            files_allowed: ['README.md'],
          }),
        } }] });
      }
      assert.match(logs.join('\n'), /Task summary:[\s\S]*Allowed files: README\.md[\s\S]*Effort:/);
      if (system.startsWith('You are the builtin reviewer seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: passingReview(body, {
            reasons: ['Diff matches the checked slice.'], security_notes: ['Documentation-only change.'] }),
        } }] });
      }
      coderTurns += 1;
      return Response.json({ choices: [coderTurns === 1 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'readme', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
        } }],
      } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Added Status.' } }] });
    },
    runTestCommand: async () => { tests += 1; return { stdout: 'pass', stderr: '' }; },
    runCommand: async (program, args, workingDirectory) => {
      assert.equal(program, 'git');
      return options.runCommand(program, args, workingDirectory);
    },
  });
  assert.equal(result.planningOnly, undefined);
  assert.equal(result.result.mode, 'llm');
  assert.equal(result.result.excellence.pass, true);
  assert.equal(result.review.verdict, 'pass');
  assert.equal(tests, 0);
  assert.match(readFileSync(path.join(result.worktreePath, 'README.md'), 'utf8'), /## Status\nReady/);
  assert.equal(readFileSync(path.join(options.target, 'README.md'), 'utf8'), '# Example\n');
  assert.match(logs.join('\n'), /Human AI-Eval|human AI-Eval/);
});

test('docs slice review repair is bounded when every review fails', async (context) => {
  const options = fixture(context);
  options.issue.body = renderIssueBody(options.issue.body, { task_class: 'docs', difficulty: 1 });
  const config = { ...llmConfig, llm: { ...llmConfig.llm, model: 'deepseek-v4.1-flash',
    base_url: 'http://192.168.1.48:8888/v1' } };
  for (const effort of ['none', 'none', 'none', 'none', 'none']) {
    let coderTurns = 0;
    const result = await runIssueWithSeats(42, { ...options, config, log: () => {},
      fetchImpl: async (_url, request) => {
        const body = JSON.parse(request.body);
        const system = body.messages[0].content;
        if (system.startsWith('You are the builtin planner seat.')) {
          assert.equal(body.reasoning_effort, 'none');
          assert.equal(body.max_tokens, 8192);
          return Response.json({ choices: [{ message: { role: 'assistant', content: JSON.stringify({
            title: options.issue.title,
            acceptance_checks: ['The requested behavior in the Ask is implemented'],
            files_allowed: ['README.md'], task_class: 'docs', difficulty: 1,
          }) } }] });
        }
        if (system.startsWith('You are the builtin reviewer seat.')) {
          return Response.json({ choices: [{ message: { role: 'assistant', content: JSON.stringify({
            verdict: 'fail', reasons: ['Needs additional evidence.'], security_notes: [],
          }) } }] });
        }
        coderTurns += 1;
        assert.equal(body.reasoning_effort, effort);
        assert.equal(body.max_tokens, 8192);
        assert.doesNotMatch(system, /## Principal|## Seat memory|implement-task/);
        const drafting = body.tools?.length;
        return Response.json({ choices: [{ finish_reason: drafting ? 'tool_calls' : 'stop',
          message: drafting ? { role: 'assistant', reasoning_content: 'PRIVATE_CODER_THINKING',
            tool_calls: [{ id: `edit-${coderTurns}`, type: 'function', function: { name: 'write_file',
              arguments: JSON.stringify({ path: 'README.md', content: `# Example\n\n## Status\n${effort} ${Date.now()}.\n` }) } }],
          } : { role: 'assistant', content: 'Changed the scoped README.', reasoning_content: 'PRIVATE_CODER_THINKING' },
        }] });
      }, runTestCommand: () => assert.fail('Docs-only changes do not run node --test'),
    });
    assert.equal(result.review.verdict, 'fail');
    assert.equal(coderTurns, 2 * (1 + maxReviewRepairs));
    assert.equal(result.reviewRepairs.length, maxReviewRepairs);
    assert.equal(result.runs.coder.metrics.effort, { low: 'l', high: 'h', max: 'x', none: '-' }[effort]);
    assert.equal(recordedCoderRun({ repoRoot: options.target, run: result.runs.coder }).line, result.runs.coder.line);
    assert.doesNotMatch(options.stderr, new RegExp(`Drafting at ${effort} effort`));
    assert.doesNotMatch(readFileSync(path.join(options.repoRoot, '.roster', 'memory', 'coder.jsonl'), 'utf8'),
      /PRIVATE_CODER_THINKING|reasoning_content/);
  }
});
