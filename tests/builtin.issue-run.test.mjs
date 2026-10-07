// Builtin seat orchestration: Ask drafts, issue runs, task metadata, worktree reuse, and cached recipes.
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
  prepareBuiltinPublication, runBuiltinAsk, runBuiltinIssue as runIssueWithSeats, runOutcomeStatus, stageReviewedFiles,
} from '../src/lib/builtin.mjs';
import { ToolAccessError } from '../src/runtime/tools.mjs';
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
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';
import { createSteeringControl } from '../src/runtime/steering.mjs';
import {
  runBuiltinIssue, example, stubConfig, llmConfig, vllmConfig, multiFileScope, git, fixture, multiFileFixture,
} from './helpers/builtin.mjs';

test('roster ask writes a local draft ask, recipe, and executable task without network', async (context) => {
  const { repoRoot } = fixture(context);
  const result = await writeAsk('Add a Status section to README.md.', {
    repoRoot, config: stubConfig, id: 'draft-1',
    fetchImpl: () => { throw new Error('stub must not contact an LLM'); },
  });
  assert.equal(result.askPath, path.join(repoRoot, '.roster', 'asks', 'draft-1.md'));
  assert.equal(readFileSync(result.askPath, 'utf8'),
    renderIssueBody('Add a Status section to README.md.'));
  assert.equal(parseRecipe(readFileSync(result.recipePath, 'utf8')).ask, 'local:draft-1');
  assert.match(readFileSync(result.taskPath, 'utf8'), /Files allowed\n- `README\.md`/);
  await assert.rejects(writeAsk('Another ask for README.md', { repoRoot, config: stubConfig, id: 'draft-1' }), /EEXIST/);
  await assert.rejects(writeAsk('', { repoRoot, config: stubConfig, id: 'draft-2' }), /Ask must be nonempty/);
});

test('builtin run reads the GitHub issue, creates a coder worktree, and stops at stub RESULT', async (context) => {
  const options = fixture(context);
  const logs = [];
  const result = await runBuiltinIssue(42, {
    ...options, config: stubConfig, log: (line) => logs.push(line),
    fetchImpl: () => { throw new Error('stub must not contact an LLM'); },
  });
  assert.equal(result.worktreePath, path.join(options.target, '.worktrees', 'issue-42'));
  assert.equal(result.logPath, path.join(options.target, '.roster', 'runs', 'roster-42-coder.log'));
  const liveLog = readFileSync(result.logPath, 'utf8');
  assert.notEqual(liveLog, options.stderr);
  assert.equal(options.stderr, '');
  assert.doesNotMatch(options.stderr, /start seat|http chat|model=|elapsed_ms=|\d{4}-\d\d-\d\dT/);
  assert.deepEqual([...liveLog.matchAll(/start seat (planner|coder|reviewer)/g)].map((match) => match[1]),
    ['planner', 'coder', 'reviewer']);
  for (const seat of ['planner', 'coder', 'reviewer']) {
    assert.match(liveLog, new RegExp(`seat ${seat} mode stub`));
    assert.match(liveLog, new RegExp(`seat ${seat} elapsed_ms=\\d+ mode=stub`));
  }
  for (const file of ['RECIPE.yml', 'TASK.md', 'RESULT.md', 'REVIEW.md']) {
    assert.ok(liveLog.includes(`wrote ${file}\n`));
  }
  assert.doesNotMatch(liveLog, /http chat\.completions|README has a Status section|## Acceptance checks/);
  const offline = await readStatus({
    issue: 42, offline: true, repoRoot: options.target, config: stubConfig,
    runCommand: () => assert.fail('Logged offline status must not call GitHub or Git'),
  });
  assert.equal(offline.runLog.lastSeat, 'reviewer');
  assert.equal(offline.runLog.lastLine, liveLog.trimEnd().split('\n').at(-1));
  assert.match(formatStatus(offline), /Last seat: reviewer/);
  assert.ok(formatStatus(offline).includes(`Last log line: ${offline.runLog.lastLine}`));
  assert.equal(git(result.worktreePath, 'branch', '--show-current'), 'issue-42');
  assert.match(readFileSync(result.assignmentPath, 'utf8'), /Issue URL: https:\/\/github.com\/example\/project\/issues\/42/);
  assert.match(readFileSync(result.assignmentPath, 'utf8'), /README has a Status section/);
  const recipe = parseRecipe(readFileSync(result.recipePath, 'utf8'));
  assert.equal(recipe.ask, 'issue:42');
  assert.deepEqual(recipe.seats.map(({ id }) => id), ['planner', 'coder', 'reviewer']);
  assert.match(readFileSync(result.taskPath, 'utf8'), /## Acceptance checks/);
  assert.equal(readFileSync(result.taskPath, 'utf8'), result.planner.task);
  assert.equal(readFileSync(result.planner.estimatePath, 'utf8'), result.planner.estimate);
  assert.match(result.planner.estimate, /difficulty: 2\nestimate_min: 15/);
  assert.equal(readFileSync(result.recipePath, 'utf8'), result.planner.recipe);
  assert.deepEqual(result.sessions, {
    planner: 'roster-42-planner', coder: 'roster-42-coder', reviewer: 'roster-42-reviewer',
  });
  assert.equal(readFileSync(result.envPath, 'utf8'),
    'AI_TASK=issue-42\nAI_SESSION=roster-42-coder\n');
  assert.match(readFileSync(result.result.resultPath, 'utf8'), /Deterministic stub only/);
  assert.match(result.result.summary, /Add Status to README/);
  assert.match(result.result.summary, /README has a Status section/);
  assert.equal(result.review.verdict, 'fail');
  assert.match(readFileSync(result.review.reviewPath, 'utf8'), /Verdict: fail/);
  assert.equal(readFileSync(path.join(options.target, 'README.md'), 'utf8'), '# Example\n');
  assert.equal(result.run, result.runs.coder);
  assert.equal(result.result.mode, 'stub');
  assert.equal(result.command, null);
  assert.match(logs.join('\n'), /Publication unavailable: set model/);
  assert.equal((logs[0].match(/AI-Run:/g) ?? []).length, 0);
  assert.deepEqual(result.runs, { planner: null, coder: null, reviewer: null });
  assert.deepEqual(options.calls.map(({ program }) => program), ['git', 'git', 'gh', 'git', 'git', 'git']);
  assert.equal(options.calls.some(({ args }) => args[0] === 'submodule'), false, 'no .gitmodules: nothing to initialize');
  assert.equal(options.calls.filter(({ program, args }) =>
    program === 'git' && args[0] === 'worktree' && args[1] === 'add').length, 1);
  assert.equal(JSON.parse(readFileSync(path.join(options.repoRoot,
    '.roster', 'memory', 'coder.jsonl'), 'utf8')).session, 'roster-42-coder');
  assert.deepEqual(JSON.parse(readFileSync(path.join(options.repoRoot,
    '.roster', 'memory', 'planner.jsonl'), 'utf8')), {
    task: 'issue-42', session: 'roster-42-planner', status: 'stub',
    summary: 'Prepared RECIPE.yml and TASK.md',
  });
  assert.deepEqual(loadLearning({ cwd: options.target }).runs, [
    { session: result.sessions.planner, task: 'issue-42', task_class: 'feat' },
    { session: result.sessions.coder, task: 'issue-42', task_class: 'feat', excellence: 'fail',
      defects: result.result.excellence.reasons },
    { session: result.sessions.reviewer, task: 'issue-42', task_class: 'feat' },
  ]);
  assert.equal(existsSync(path.join(options.target, '.roster', 'evals.jsonl')), false);
  const { records: provenance } = await openProvenanceStore(path.join(options.target, '.git', 'roster', 'provenance')).readAll();
  const seatEvents = provenance.filter((record) => record.event === 'session')
    .map(({ sessionId, runId, payload }) => ({ sessionId, runId, seat: payload.seat, issue: payload.issue, outcome: payload.outcome }));
  assert.deepEqual(seatEvents.map(({ seat }) => seat).sort(), ['coder', 'planner', 'reviewer']);
  assert.equal(new Set(seatEvents.map(({ runId }) => runId)).size, 1, 'one durable run id per process run');
  assert.ok(seatEvents.every(({ issue }) => issue === 42));
  assert.equal(seatEvents.find(({ seat }) => seat === 'coder').outcome, 'fail');
  assert.doesNotThrow(() => git(options.target, 'check-ignore', '--quiet',
    '.roster/runs/runs.jsonl'));
  await assert.rejects(stageReviewedFiles(result.worktreePath, ['README.md']),
    /No reviewed task files changed/);
});

test('issue body task metadata reaches TASK.md and ESTIMATE.md before coder/reviewer', async (context) => {
  const options = fixture(context);
  options.issue.title = 'fix(cli): Update status';
  options.issue.body = renderIssueBody(options.issue.body, {
    task_class: 'fix', difficulty: 4, estimate_min: 35,
  });
  const result = await runIssueWithSeats(42, {
    ...options, config: stubConfig, log: () => {},
    fetchImpl: () => assert.fail('Stub must not call an LLM'),
  });
  assert.equal(result.ask, 'Add a Status section to README.md.\n\n## Acceptance checks\n' +
    '- node --test exits 0\n- README has a Status section\n\n## Files allowed\n- `README.md`');
  assert.deepEqual(result.metadata, { task_class: 'fix', difficulty: 4, estimate_min: 35 });
  assert.match(result.planner.task, /difficulty: 4\nestimate_min: 35\ntask_class: fix\n/);
  assert.match(result.planner.estimate, /difficulty: 4\nestimate_min: 35\ntask_class: fix\n/);
  assert.equal(result.review.verdict, 'fail');
});

test('rerunning an issue reuses its worktree and preserves prior run artifacts without changing app code', async (context) => {
  const options = fixture(context);
  const initial = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  const oldTask = readFileSync(initial.taskPath, 'utf8');
  const oldResult = readFileSync(initial.result.resultPath, 'utf8');
  const oldEnv = readFileSync(initial.envPath, 'utf8');
  writeFileSync(path.join(initial.worktreePath, 'README.md'), '# Operator change\n');
  const rerun = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    fetchImpl: () => assert.fail('Stub rerun must not call an endpoint') });
  assert.equal(rerun.reused, true);
  assert.equal(rerun.worktreePath, initial.worktreePath);
  assert.equal(git(rerun.worktreePath, 'branch', '--show-current'), 'issue-42');
  assert.equal(readFileSync(rerun.taskPath, 'utf8'), oldTask);
  assert.equal(readFileSync(path.join(rerun.archivePath, 'RESULT.md'), 'utf8'), oldResult);
  assert.equal(rerun.planner.reused, true);
  assert.equal(readFileSync(rerun.envPath, 'utf8'), oldEnv);
  assert.equal(readFileSync(path.join(rerun.worktreePath, 'README.md'), 'utf8'), '# Operator change\n');
  assert.equal(options.calls.filter(({ args }) => args[0] === 'worktree' && args[1] === 'add').length, 1);
});

test('a valid existing issue-92 RECIPE/TASK skips the planner and starts the scoped coder', async (context) => {
  const options = fixture(context);
  const ask = 'Add a one-line Status section to README.md';
  options.issue.number = 92;
  options.issue.title = 'Add a one-line Status section to `README.md`';
  options.issue.body = renderIssueBody('Different body lead; the issue title identifies the request.');
  options.issue.url = 'https://github.com/example/project/issues/92';
  const worktree = path.join(options.target, '.worktrees', 'issue-92');
  git(options.target, 'worktree', 'add', '-b', 'issue-92', worktree);
  const task = readFileSync(new URL('./fixtures/planner-task-92.md', import.meta.url), 'utf8');
  const recipe = `title: ${ask}\nacceptance_checks:\n  - node --test exits 0\nfiles_allowed:\n  - README.md\n`;
  writeFileSync(path.join(worktree, 'TASK.md'), task);
  writeFileSync(path.join(worktree, 'RECIPE.yml'), recipe);
  writeFileSync(path.join(worktree, 'ESTIMATE.md'), '# Previous estimate\n');
  const logs = [];
  let calls = 0;
  const result = await runBuiltinIssue(92, {
    ...options, config: llmConfig, log: (text) => logs.push(text),
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.doesNotMatch(body.messages[0].content, /builtin planner seat/);
      assert.match(body.messages[0].content, /## Issue Ask[\s\S]*# Outcome:[\s\S]*## Scope/);
      return Response.json({ choices: [calls === 1 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'status', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nActive.\n' }),
        } }],
      } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Added a one-line Status section.' } }] });
    },
    runTestCommand: () => assert.fail('Documentation-only changes do not run node --test'),
  });
  assert.equal(calls, 2, 'Only coder turns should reach this mock');
  assert.equal(result.askKind, 'slice');
  assert.equal(result.planner.reused, true);
  assert.equal(result.reused, true);
  assert.equal(result.worktreePath, worktree);
  assert.equal(result.runs.planner, null);
  assert.equal(result.result.excellence.pass, true);
  assert.equal(result.review.verdict, 'pass');
  assert.equal(readFileSync(result.taskPath, 'utf8'), task);
  assert.equal(parseRecipe(readFileSync(result.recipePath, 'utf8')).ask, 'issue:92');
  assert.equal(readFileSync(path.join(result.archivePath, 'RECIPE.yml'), 'utf8'), recipe);
  assert.equal(readFileSync(path.join(result.archivePath, 'ESTIMATE.md'), 'utf8'), '# Previous estimate\n');
  assert.equal(options.calls.some(({ args }) => args[0] === 'worktree' && args[1] === 'add'), false);
  assert.deepEqual(loadLearning({ cwd: options.target }).runs.map(({ session }) => session),
    ['roster-92-coder', 'roster-92-reviewer']);
  assert.doesNotMatch(readFileSync(result.logPath, 'utf8'), /start seat planner/);
  assert.match(readFileSync(result.logPath, 'utf8'), /start seat coder/);
  assert.doesNotMatch(options.stderr, /Writing the plan|Drafting the change/);
  assert.match(logs.join('\n'), /planner skipped artifacts valid/);
});

test('an invalid cached recipe is preserved in the archive and replanned, not blindly reused', async (context) => {
  const options = fixture(context);
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  git(options.target, 'worktree', 'add', '-b', 'issue-42', worktree);
  const task = planStub(options.issue.body, { reference: 'issue:42' }).task;
  const wrongRecipe = planStub(options.issue.body, { reference: 'issue:99' }).recipe;
  writeFileSync(path.join(worktree, 'TASK.md'), task);
  writeFileSync(path.join(worktree, 'RECIPE.yml'), wrongRecipe);
  const logs = [];
  const result = await runBuiltinIssue(42, { ...options, config: stubConfig,
    log: (text) => logs.push(text), fetchImpl: () => assert.fail('Stub replanning must not use a model') });
  assert.equal(result.planner.reused, undefined);
  assert.equal(readFileSync(path.join(result.archivePath, 'RECIPE.yml'), 'utf8'), wrongRecipe);
  assert.equal(parseRecipe(result.planner.recipe).ask, 'issue:42');
  assert.match(logs.join('\n'), /do not validate for this issue; replanning is required/);
});

test('an inferred-scope slice continues after the task summary without a second run', async (context) => {
  const options = fixture(context);
  options.issue.body = 'Add a one-line Status section to README.md.';
  const logs = [];
  const first = await runBuiltinIssue(42, { ...options, config: stubConfig,
    log: (message) => {
      logs.push(message);
      if (message.startsWith('Task summary:')) {
        assert.equal(existsSync(path.join(options.target, '.worktrees', 'issue-42', 'RESULT.md')), false);
      }
    },
    fetchImpl: () => assert.fail('Stub planning does not call a model') });
  assert.equal(first.planningOnly, undefined);
  assert.equal(existsSync(path.join(first.worktreePath, 'RESULT.md')), true);
  assert.equal(existsSync(path.join(first.worktreePath, 'REVIEW.md')), true);
  assert.match(logs.join('\n'), /Task summary:\nOutcome: .+\nAllowed files: README\.md\nChecks:\n- The requested behavior in the Ask is implemented\nEffort: [lmhx]/);
  assert.doesNotMatch(logs.join('\n'), /then \/run 42/);
  const next = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  assert.equal(next.planner.reused, true);
  assert.equal(next.planningOnly, undefined);
  assert.equal(next.result.mode, 'stub');
});

test('an App-credentialed issue run reports claim and outcome on the issue without failing on board errors', async (context) => {
  const options = fixture(context);
  const env = { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' };
  const statuses = [];
  const logs = [];
  const issueStatus = async ({ issue, status, detail, repoRoot }) => {
    statuses.push({ number: issue.number, url: issue.url, status, detail, repoRoot });
    if (status === 'in-progress') throw new Error('GitHub issue API request failed (HTTP 502)');
    return { claimed: false };
  };
  const result = await runBuiltinIssue(42, { ...options, env, config: stubConfig, issueStatus,
    log: (line) => logs.push(line), fetchImpl: () => { throw new Error('stub must not contact an LLM'); } });
  assert.equal(result.failed, false);
  assert.deepEqual(statuses.map(({ status }) => status), ['in-progress', result.review?.verdict === 'pass' ? 'review' : 'blocked']);
  assert.equal(statuses[0].url, options.issue.url);
  assert.equal(statuses[0].repoRoot, options.target);
  assert.deepEqual(statuses[0].detail, ['Branch: issue-42', 'Seats: planner,coder,reviewer']);
  assert.ok(logs.includes('Issue #42 status not updated (in-progress): GitHub issue API request failed (HTTP 502)'));

  statuses.length = 0;
  await assert.rejects(runBuiltinIssue(42, { ...options, env, config: stubConfig, issueStatus, log: () => {},
    onPrepared() { throw new Error('operator hook failed'); } }), /operator hook failed/);
  assert.deepEqual(statuses.map(({ status, detail }) => [status, detail]),
    [['blocked', ['Run stopped: operator hook failed']]]);

  statuses.length = 0;
  await runBuiltinIssue(42, { ...options, config: stubConfig, issueStatus, log: () => {} });
  assert.deepEqual(statuses, [], 'without App credentials the board is never touched');
});

test('a claimed issue warns of another agent on a fresh worktree and reports a resume on a reused one', async (context) => {
  const options = fixture(context);
  const env = { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' };
  const logs = [];
  const issueStatus = async ({ status }) => ({ claimed: status === 'in-progress' });
  const run = () => runBuiltinIssue(42, { ...options, env, config: stubConfig, issueStatus,
    log: (line) => logs.push(line), fetchImpl: () => { throw new Error('stub must not contact an LLM'); } });
  await run();
  assert.ok(logs.some((line) => /Issue #42 was already labeled roster:in-progress; another agent/.test(line)), logs.join('\n'));
  logs.length = 0;
  await run();
  assert.ok(logs.some((line) => /Resuming #42: its roster:in-progress claim and worktree are from an earlier run/.test(line)), logs.join('\n'));
});

test('a feature whose child slices are all closed reports review, while open children still wait on a human', () => {
  const rows = [{ wave: 1, issue: 251, state: 'done' }, { wave: 2, issue: 253, state: 'done' }];
  const [status, detail] = runOutcomeStatus({ planningOnly: true, waves: rows }, { published: false });
  assert.equal(status, 'review');
  assert.match(detail.join('\n'), /Every child issue is closed: #251, #253\.\nClose this parent after human AI-Eval/);
  const open = runOutcomeStatus({ planningOnly: true, waves: [rows[0], { ...rows[1], state: 'review' }] }, { published: false });
  assert.equal(open[0], 'blocked');
  assert.equal(runOutcomeStatus({ planningOnly: true, waves: [] }, { published: false })[0], 'blocked');
});
