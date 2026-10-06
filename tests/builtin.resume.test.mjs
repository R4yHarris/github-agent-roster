// Builtin seat orchestration: Debug logging, contracts submodule, retry, resume, plan mode, waves, and steering.
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

test('opt-in debug logging reaches planner, coder and reviewer without changing the human summary', async (context) => {
  const options = fixture(context);
  const debug = createDebugLog({ env: {}, enabled: true, session: 'builtin-debug' });
  const result = await runBuiltinIssue(42, { ...options, config: stubConfig, debug, log: () => {} });
  const rows = readFileSync(debug.path, 'utf8').trim().split('\n').map((line) => JSON.parse(line));
  assert.deepEqual([...new Set(rows.map(({ seat }) => seat))], ['planner', 'coder', 'reviewer']);
  assert.ok(rows.every(({ issue }) => issue === 42));
  assert.doesNotMatch(options.stderr, /"phase":|"path_class":/);
  assert.equal(result.result.mode, 'stub');
  assert.equal(git(options.target, 'check-ignore', debug.path).length > 0, true);
});

test('new issue and local-ask worktrees contain the initialized contracts publisher', async (context) => {
  const options = fixture(context);
  git(options.contracts, 'init', '-b', 'main');
  git(options.contracts, 'add', '--all');
  git(options.contracts, '-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '-m', 'Fixture contracts');
  git(options.target, '-c', 'protocol.file.allow=always', 'submodule', 'add', options.contracts,
    'vendor/github-agent-contracts');
  git(options.target, '-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '-am', 'Fixture submodule');
  const original = options.runCommand;
  const runCommand = async (program, args, cwd) => {
    if (program === 'git' && args[0] === 'submodule') {
      return git(cwd, '-c', 'protocol.file.allow=always', ...args);
    }
    return original(program, args, cwd);
  };
  const issue = await runBuiltinIssue(42, { ...options, runCommand, config: stubConfig, log: () => {} });
  const local = await runBuiltinAsk('Add a Status section to README.md.', {
    ...options, runCommand, config: stubConfig, log: () => {},
  });
  for (const run of [issue, local]) {
    const publisher = path.join(run.worktreePath, 'vendor', 'github-agent-contracts', 'scripts', 'agent-pr.mjs');
    assert.equal(existsSync(publisher), true);
    assert.equal(readFileSync(publisher, 'utf8').replaceAll('\r\n', '\n'), 'export {};\n');
    assert.doesNotMatch(git(run.worktreePath, 'submodule', 'status'), /^-/);
  }
});

test('local retry reuses its registered worktree, cached planner handoff and initialized submodule', async (context) => {
  const options = fixture(context);
  const ask = 'Add a Status section to README.md.';
  const first = await runBuiltinAsk(ask, { ...options, config: stubConfig, log: () => {} });
  const second = await runBuiltinAsk(ask, { ...options, config: stubConfig, preparedRun: first, log: () => {} });
  assert.equal(second.worktreePath, first.worktreePath);
  assert.equal(second.planner.reused, true);
  assert.equal(second.planningOnly, undefined);
  assert.ok(second.archivePath);
  assert.equal(options.calls.filter(({ program, args }) =>
    program === 'git' && args[0] === 'worktree' && args[1] === 'add').length, 1);
  await assert.rejects(runBuiltinAsk('Change a different Ask in README.md.', {
    ...options, config: stubConfig, preparedRun: second, log: () => {},
  }), /unchanged prepared Ask/);
});

test('a confirmed issue resumes its prepared handoff without another gh view or worktree add', async (context) => {
  const options = fixture(context);
  const first = await runBuiltinIssue(42, { ...options, config: stubConfig, confirm: true, log: () => {} });
  const initialGh = options.calls.filter(({ program }) => program === 'gh').length;
  const second = await runBuiltinIssue(42, { ...options, config: stubConfig, preparedRun: first, log: () => {} });
  assert.equal(second.worktreePath, first.worktreePath);
  assert.equal(second.planner.reused, true);
  assert.equal(second.planningOnly, undefined);
  assert.equal(second.result.mode, 'stub');
  assert.equal(options.calls.filter(({ program }) => program === 'gh').length, initialGh);
  assert.equal(options.calls.filter(({ args }) => args[0] === 'worktree' && args[1] === 'add').length, 1);
});

test('plan mode stays PLAN-only until a human accepts and resumes the same slice worktree', async (context) => {
  const options = fixture(context);
  const ask = 'Add a Status section to README.md.';
  const first = await runBuiltinAsk(ask, { ...options, config: stubConfig, planMode: true, log: () => {} });
  assert.equal(first.planMode, true);
  assert.equal(first.planningOnly, true);
  assert.equal(existsSync(path.join(first.worktreePath, 'TASK.md')), false);
  assert.equal(existsSync(path.join(first.worktreePath, 'RECIPE.yml')), false);
  assert.equal(existsSync(path.join(first.worktreePath, 'RESULT.md')), false);
  const second = await runBuiltinAsk(ask, { ...options, config: stubConfig, preparedRun: first,
    acceptPlan: true, log: () => {} });
  assert.equal(second.worktreePath, first.worktreePath);
  assert.equal(second.planner.acceptedPlan, true);
  assert.equal(second.planningOnly, undefined);
  assert.equal(existsSync(first.planPath), true);
  assert.equal(second.result.mode, 'stub');
});

test('a locally reconstructed resume handle reuses valid TASK with no planner or worktree creation', async (context) => {
  const options = fixture(context);
  const first = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  const prepared = await readLocalRun({ number: 42, cwd: options.target, config: stubConfig, env: options.env });
  const before = options.calls.length;
  const second = await runBuiltinIssue(42, { ...options, config: stubConfig, preparedRun: prepared, log: () => {},
    fetchImpl: () => assert.fail('A valid stub handoff must not request a planner model') });
  assert.equal(second.worktreePath, first.worktreePath);
  assert.equal(second.planner.reused, true);
  assert.equal(options.calls.slice(before).some(({ program, args }) =>
    program === 'gh' || args[0] === 'worktree' && args[1] === 'add'), false);
});

test('an open earlier wave blocks the later issue before a worktree or coder starts', async (context) => {
  const options = fixture(context);
  options.issue.labels = [{ name: 'wave:2' }];
  const original = options.runCommand;
  await assert.rejects(runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    runCommand: async (program, args, cwd) => program === 'gh' && args[0] === 'issue' && args[1] === 'list'
      ? JSON.stringify([{ number: 41, title: 'Wave 1' }]) : original(program, args, cwd),
  }), /Wave 2 is blocked/);
  assert.equal(options.calls.some(({ args }) => args[0] === 'worktree' && args[1] === 'add'), false);
});

test('resume preserves manual wave labels and cannot bypass a newly reopened earlier wave', async (context) => {
  const options = fixture(context);
  options.issue.labels = [{ name: 'wave:2' }];
  const original = options.runCommand;
  await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    runCommand: async (program, args, cwd) => program === 'gh' && args[0] === 'issue' && args[1] === 'list'
      ? '[]' : original(program, args, cwd) });
  const prepared = await readLocalRun({ number: 42, cwd: options.target, config: stubConfig, env: options.env });
  assert.deepEqual(prepared.issue.labels, [{ name: 'wave:2' }]);
  await assert.rejects(runBuiltinIssue(42, { ...options, config: stubConfig, preparedRun: prepared, log: () => {},
    runCommand: async (program, args, cwd) => program === 'gh' && args[0] === 'issue' && args[1] === 'list'
      ? JSON.stringify([{ number: 41 }]) : original(program, args, cwd) }), /Wave 2 is blocked/);
});

test('a live builtin coder can be steered into its next scoped instruction without a second run', async (context) => {
  const options = fixture(context);
  const steeringControl = createSteeringControl();
  let coderCalls = 0;
  let firstSignal;
  const pending = runBuiltinIssue(42, { ...options, config: llmConfig, steeringControl, log: () => {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      if (body.messages[0].content.startsWith('You are the builtin planner seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: JSON.stringify({ title: 'Add Status',
            acceptance_checks: ['The requested behavior in the Ask is implemented'],
            files_allowed: ['README.md'] }) } }] });
      }
      if (body.messages[0].content.startsWith('You are the builtin reviewer seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: passingReview(body, { reasons: ['The scoped change matches the task.'] }) } }] });
      }
      coderCalls += 1;
      if (coderCalls === 1) { firstSignal = request.signal; return new Promise(() => {}); }
      if (coderCalls === 2) {
        assert.match(body.messages.at(-1).content, /Human steering[\s\S]*Keep the change scoped/);
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant',
          tool_calls: [{ id: 'scoped', type: 'function', function: { name: 'write_file',
            arguments: '{"path":"README.md","content":"# Example\\n\\n## Status\\nReady.\\n"}' } }] } }] });
      }
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Added Status.' } }] });
    }, runTestCommand: () => assert.fail('Documentation-only changes do not run node --test') });
  for (let index = 0; index < 1000 && !firstSignal; index += 1) await new Promise((resolve) => setTimeout(resolve, 10));
  assert.ok(firstSignal && steeringControl.waiting);
  steeringControl.steer('Keep the change scoped to README.md.');
  const run = await pending;
  assert.equal(firstSignal.aborted, true);
  assert.equal(coderCalls, 3);
  assert.equal(run.review.verdict, 'pass');
  assert.match(readFileSync(run.taskPath, 'utf8'), /Files allowed\n- `README\.md`/);
});
