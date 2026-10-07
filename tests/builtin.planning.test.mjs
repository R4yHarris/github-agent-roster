// Builtin seat orchestration: Planner timeouts, initiatives, clarify, branch reuse, and planner reruns.
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

test('cold endpoint timeout preserves a valid TASK and retry skips planner rather than marking the TASK bad', async (context) => {
  const options = fixture(context);
  const initial = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  const task = readFileSync(initial.taskPath, 'utf8');
  const recipe = readFileSync(initial.recipePath, 'utf8');
  const coldConfig = { ...llmConfig, llm: { ...llmConfig.llm,
    base_url: 'http://192.168.1.48:8888/v1', request_timeout_ms: 10 } };
  await assert.rejects(runBuiltinIssue(42, { ...options, config: coldConfig, log: () => {},
    fetchImpl: () => new Promise(() => {}),
    runTestCommand: () => assert.fail('Timed-out inference cannot run tests'),
  }), (error) => {
    assert.equal(error.result.timedOut, true);
    assert.equal(error.result.review, undefined);
    return /Cold-start:[\s\S]*host may still be warming[\s\S]*not a bad TASK[\s\S]*Retry: roster run --issue 42/.test(error.message);
  });
  assert.equal(readFileSync(initial.taskPath, 'utf8'), task);
  assert.equal(readFileSync(initial.recipePath, 'utf8'), recipe);
  assert.doesNotMatch(task, /Planning failure/);
  assert.match(options.stderr, /The model did not answer in time\. It may still be waking\./);
  assert.match(readFileSync(initial.logPath, 'utf8'), /host may still be warming[\s\S]*Retry: roster run --issue 42/);
  const logs = [];
  let retryTurns = 0;
  const retried = await runBuiltinIssue(42, { ...options, config: llmConfig, log: (text) => logs.push(text),
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      assert.doesNotMatch(body.messages[0].content, /builtin planner seat/);
      if (body.messages[0].content.startsWith('You are the builtin reviewer seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: passingReview(body),
        } }] });
      }
      retryTurns += 1;
      return retryTurns === 1
        ? Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'rewrite', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({
              path: 'README.md', content: readFileSync(path.join(initial.worktreePath, 'README.md'), 'utf8'),
            }),
          } }],
        } }] })
        : Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: 'No change needed.',
        } }] });
    },
    runTestCommand: () => assert.fail('Documentation-only changes do not run node --test'),
  });
  assert.equal(retried.planner.reused, true);
  assert.equal(retried.failed, false);
  assert.equal(readFileSync(initial.taskPath, 'utf8'), task);
  assert.match(logs.join('\n'), /planner skipped artifacts valid/);
});

test('README one-liner runs the sequential slice seats with minimum pack even at feat difficulty4', async (context) => {
  const options = fixture(context);
  options.issue.body = renderIssueBody(options.issue.body, { task_class: 'feat', difficulty: 4 });
  const result = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    fetchImpl: () => assert.fail('Stub slice must not call a model') });
  assert.equal(result.askKind, 'slice');
  assert.deepEqual([...readFileSync(result.logPath, 'utf8').matchAll(/start seat (planner|coder|reviewer)/g)].map((match) => match[1]),
    ['planner', 'coder', 'reviewer']);
  assert.equal(result.result.stages.includes('research'), false);
  assert.equal(existsSync(path.join(result.worktreePath, 'RESEARCH.md')), false);
  const pack = readFileSync(result.result.contextPath, 'utf8');
  assert.ok(pack.length < 3500, `Slice minimum pack grew to ${pack.length} characters`);
  assert.match(pack, /## Issue Ask[\s\S]*# Outcome:/);
  assert.match(pack, /read-before-write[\s\S]*small-diff/);
  assert.doesNotMatch(pack, /## Principal|## AGENTS|## Seat memory|## Prior feedback|implement-task/);
  assert.equal(existsSync(path.join(result.worktreePath, 'PLAN.md')), false);
});

test('build an orchestrator produces initiative PLAN only and cannot edit README or publish', async (context) => {
  const options = fixture(context);
  options.issue.title = 'build an orchestrator';
  options.issue.body = renderIssueBody('build an orchestrator');
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  git(options.target, 'worktree', 'add', '-b', 'issue-42', worktree);
  const before = readFileSync(path.join(worktree, 'README.md'));
  const result = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    fetchImpl: () => assert.fail('Stub initiative must not contact a model'),
    runTestCommand: () => assert.fail('Initiative must not run coder tests'),
    publisher: () => assert.fail('Initiative must not publish'),
    issueCommenter: () => assert.fail('Initiative must not create issues or PR comments'),
  });
  assert.equal(result.askKind, 'initiative');
  assert.equal(result.planningOnly, true);
  assert.equal(result.command, null);
  assert.equal(result.runs.coder, null);
  assert.equal(result.runs.reviewer, null);
  assert.equal(result.result, undefined);
  assert.deepEqual(readFileSync(path.join(result.worktreePath, 'README.md')), before);
  assert.equal(git(result.worktreePath, 'diff', '--name-only'), '');
  for (const name of ['TASK.md', 'RECIPE.yml', 'ESTIMATE.md', 'CONTEXT.md', 'RESEARCH.md', 'RESULT.md', 'REVIEW.md']) {
    assert.equal(existsSync(path.join(result.worktreePath, name)), false, name);
  }
  const plan = readFileSync(result.planPath, 'utf8');
  assert.match(plan, /Ask kind: initiative[\s\S]*## Outcomes[\s\S]*## Waves[\s\S]*## Child issue drafts/);
  assert.match(plan, /Labels: `wave:1`/);
  assert.doesNotMatch(plan, /README\.md|\*\*\/\*/);
  assert.match(readFileSync(result.logPath, 'utf8'), /start seat planner[\s\S]*tool write_file path="PLAN\.md"[\s\S]*wrote PLAN\.md/);
  assert.doesNotMatch(options.stderr, /Writing the plan|Saving PLAN\.md\./);
  assert.doesNotMatch(options.stderr, /Drafting the change|Checking the diff|Running tests|build an orchestrator/);
  assert.deepEqual(loadLearning({ cwd: options.target }).runs.map(({ session }) => session), ['roster-42-planner']);
  const status = await readStatus({ issue: 42, offline: true, repoRoot: options.target, config: stubConfig });
  assert.equal(status.artifacts['PLAN.md'], true);
  assert.equal(status.runLog.lastSeat, 'planner');
  assert.ok(status.runLog.lines.some((line) => line.endsWith('wrote PLAN.md')));
  assert.match(formatStatus(status), /PLAN\.md=yes/);
  await assert.rejects(prepareBuiltinPublication(result, { config: stubConfig, skipReview: true, env: options.env }),
    /Planning-only output is not code/);
});

test('an initiative cannot consume even a valid cached TASK and prior PLAN is archived on repeat planning', async (context) => {
  const options = fixture(context);
  options.issue.title = 'build an orchestrator';
  options.issue.body = 'build an orchestrator.\n\n## Allowed files\n- `README.md`';
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  git(options.target, 'worktree', 'add', '-b', 'issue-42', worktree);
  const before = readFileSync(path.join(worktree, 'README.md'));
  const cached = planStub(options.issue.body, { title: options.issue.title, reference: 'issue:42' });
  writeFileSync(path.join(worktree, 'TASK.md'), cached.task);
  writeFileSync(path.join(worktree, 'RECIPE.yml'), cached.recipe);
  const first = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    runTestCommand: () => assert.fail('Cached initiative cannot become a coder task') });
  assert.equal(first.askKind, 'initiative');
  assert.equal(first.planner.reused, undefined);
  assert.equal(readFileSync(path.join(first.archivePath, 'TASK.md'), 'utf8'), cached.task);
  assert.equal(existsSync(path.join(worktree, 'TASK.md')), false);
  const firstPlan = readFileSync(first.planPath, 'utf8');
  const again = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    runTestCommand: () => assert.fail('Repeating initiative must stay planning-only') });
  assert.equal(again.reused, true);
  assert.equal(again.planningOnly, true);
  assert.equal(readFileSync(path.join(again.archivePath, 'PLAN.md'), 'utf8'), firstPlan);
  assert.deepEqual(readFileSync(path.join(worktree, 'README.md')), before);
  assert.doesNotMatch(options.stderr, /Drafting the change|Checking the diff/);
  assert.doesNotMatch(readFileSync(again.logPath, 'utf8'), /start seat coder|start seat reviewer/);
});

test('feature planner writes five child issue drafts with wave labels and --confirm stops before opening or coding', async (context) => {
  const options = fixture(context);
  options.issue.title = 'Implement a profile feature';
  options.issue.body = 'Implement a profile feature.\n\n## Allowed files\n- `README.md`';
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  git(options.target, 'worktree', 'add', '-b', 'issue-42', worktree);
  const before = readFileSync(path.join(worktree, 'README.md'));
  let calls = 0;
  const result = await runBuiltinIssue(42, { ...options, config: llmConfig, confirm: true, log: () => {},
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.tools, undefined);
      assert.match(body.messages[0].content, /feature planner seat/);
      return Response.json({ model: 'served-planner', usage: { prompt_tokens: 100, completion_tokens: 40 },
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
          outcomes: ['A profile feature works'],
          issues: Array.from({ length: 5 }, (_, index) => ({
            title: `Profile slice ${index + 1}`, outcome: `Profile outcome ${index + 1}`, wave: index + 1,
            acceptance_checks: ['Outcome is verified'], files_allowed: ['README.md'],
          })),
        }) } }] });
    },
    runTestCommand: () => assert.fail('Feature cannot call coder tests'),
    publisher: () => assert.fail('Feature cannot publish'),
    issueCommenter: () => assert.fail('Feature children remain drafts'),
  });
  assert.equal(calls, 1);
  assert.equal(result.askKind, 'feature');
  assert.equal(result.planningOnly, true);
  assert.equal(result.continueWith, undefined);
  assert.equal(result.runs.planner.metrics.model, 'served-planner');
  assert.equal(result.runs.coder, null);
  const plan = readFileSync(result.planPath, 'utf8');
  assert.equal([...plan.matchAll(/^### Draft \d+:/gm)].length, 5);
  assert.match(plan, /Labels: `wave:5`/);
  assert.deepEqual(readFileSync(path.join(result.worktreePath, 'README.md')), before);
  assert.doesNotMatch(options.stderr, /Drafting the change|Checking the diff|Profile outcome/);
  assert.doesNotMatch(readFileSync(result.logPath, 'utf8'), /start seat coder|start seat reviewer/);
  assert.equal(options.calls.filter(({ program }) => program === 'gh').length, 1);
});

test('an issue feature opens linked wave children, continues into the first slice, and reuses its PLAN on rerun', async (context) => {
  const options = fixture(context);
  const parent = { ...options.issue, title: 'Implement a profile feature',
    body: 'Implement a profile feature across the README and smoke test.' };
  const children = [];
  const labels = [];
  options.runCommand = async (program, args, workingDirectory) => {
    options.calls.push({ program, args, workingDirectory });
    if (program !== 'gh') return git(workingDirectory, ...args);
    if (args[0] === 'issue' && args[1] === 'view') {
      const number = Number(args[2]);
      return JSON.stringify(number === 42 ? parent : children.find((issue) => issue.number === number));
    }
    if (args[0] === 'issue' && args[1] === 'list') return JSON.stringify(children);
    if (args[0] === 'label' && args[1] === 'list') return JSON.stringify(labels.map((name) => ({ name })));
    if (args[0] === 'label' && args[1] === 'create') { labels.push(args[2]); return ''; }
    if (args[0] === 'issue' && args[1] === 'create') {
      const issue = { number: 100 + children.length, title: args[args.indexOf('--title') + 1], state: 'OPEN',
        body: args[args.indexOf('--body') + 1], labels: [{ name: args[args.indexOf('--label') + 1] }],
        url: `https://github.com/example/project/issues/${100 + children.length}` };
      children.push(issue);
      return `${issue.url}\n`;
    }
    if (args[0] === 'pr' && args[1] === 'list') return '[]';
    throw new Error(`Unexpected gh ${args.join(' ')}`);
  };
  let planners = 0;
  const fetchImpl = async (_url, request) => {
    const body = JSON.parse(request.body);
    const system = body.messages[0].content;
    if (/feature planner seat/.test(system)) {
      planners += 1;
      assert.ok(JSON.parse(body.messages[1].content).repository_files.includes('README.md'));
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
        outcomes: ['A profile feature works'], issues: [
          { title: 'Document the profile', outcome: 'README documents the profile', wave: 1,
            acceptance_checks: ['README has a Profile section'], files_allowed: ['README.md'] },
          { title: 'Test the profile', outcome: 'Smoke test covers the profile', wave: 2,
            acceptance_checks: ['node --test exits 0'], files_allowed: ['smoke.test.mjs'] },
        ] }) } }] });
    }
    if (system.startsWith('You are the builtin reviewer seat.')) {
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: passingReview(body) } }] });
    }
    if (system.startsWith('You are the builtin planner seat.')) {
      const draft = planStub(children[0].body, { reference: 'issue:100', title: children[0].title });
      return body.messages.at(-1).role === 'tool'
        ? Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Task ready.' } }] })
        : Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{
          id: 'task', type: 'function', function: { name: 'write_file',
            arguments: JSON.stringify({ path: 'TASK.md', content: draft.task }) } }] } }] });
    }
    return body.messages.some((message) => message.role === 'tool')
      ? Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Documented the profile.' } }] })
      : Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{
        id: 'profile', type: 'function', function: { name: 'write_file',
          arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Profile\n\nProfiles work.\n' }) },
      }] } }] });
  };
  const logs = [];
  const result = await runBuiltinIssue(42, { ...options, config: llmConfig, log: (text) => logs.push(text), fetchImpl,
    runTestCommand: () => assert.fail('Documentation-only child does not run node --test') });
  assert.equal(planners, 1);
  assert.deepEqual(children.map(({ number, title }) => [number, title]), [[100, 'Document the profile'], [101, 'Test the profile']]);
  assert.match(children[0].body, /^Parent: #42$/m);
  assert.equal(result.parent.issue, 42);
  assert.deepEqual(result.parent.waves.map(({ issue, state }) => [issue, state]), [[100, 'todo'], [101, 'blocked']]);
  assert.equal(result.issue.number, 100);
  assert.equal(result.askKind, 'slice');
  assert.equal(result.failed, false);
  assert.equal(result.review.verdict, 'pass');
  assert.match(readFileSync(path.join(result.worktreePath, 'README.md'), 'utf8'), /## Profile/);
  assert.match(logs.join('\n'), /Continuing feature #42 with wave 1 child #100: Document the profile/);

  const again = await runBuiltinIssue(42, { ...options, config: llmConfig, confirm: true, log: (text) => logs.push(text),
    fetchImpl: () => assert.fail('An executable PLAN is reused, not re-planned') });
  assert.equal(again.planner.reused, true);
  assert.equal(children.length, 2);
  assert.match(logs.join('\n'), /Reusing executable feature PLAN/);
});

test('an Ask without scope or planning intent stops for clarify before any seat', async (context) => {
  const options = fixture(context);
  options.issue.title = 'Improve things';
  options.issue.body = 'Improve things';
  const messages = [];
  const result = await runBuiltinIssue(42, { ...options, config: stubConfig, log: (message) => messages.push(message),
    fetchImpl: () => assert.fail('Clarify must not call a model'),
    runTestCommand: () => assert.fail('Clarify must not call tests'),
  });
  assert.equal(result.askKind, 'clarify');
  assert.equal(result.planningOnly, true);
  assert.deepEqual(result.runs, { planner: null, coder: null, reviewer: null });
  assert.match(messages.join('\n'), /Ask kind: clarify[\s\S]*Clarify one concrete outcome/);
  assert.equal(options.stderr, '');
  assert.equal(existsSync(path.join(result.worktreePath, 'PLAN.md')), false);
  assert.equal(existsSync(path.join(result.worktreePath, 'TASK.md')), false);
});

test('an existing branch is reused when its issue worktree needs to be created', async (context) => {
  const options = fixture(context);
  git(options.target, 'branch', 'issue-42');
  const result = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  assert.equal(git(result.worktreePath, 'branch', '--show-current'), 'issue-42');
  const add = options.calls.find(({ args }) => args[0] === 'worktree' && args[1] === 'add');
  assert.equal(add.args.includes('-b'), false);
});

test('garbage planner arguments retry once then stop before coder, reviewer, tests, or publishing', async (context) => {
  const options = fixture(context);
  const logs = [];
  let calls = 0;
  let originalReadme;
  const result = await runBuiltinIssue(42, {
    ...options, config: llmConfig, publish: true, log: (text) => logs.push(text),
    env: { ...options.env, ROSTER_API_KEY: 'test-key', GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
    fetchImpl: async (_url, request) => {
      calls += 1;
      if (calls === 1) originalReadme = readFileSync(path.join(options.target, '.worktrees', 'issue-42', 'README.md'), 'utf8');
      const body = JSON.parse(request.body);
      assert.match(body.messages[0].content, /builtin planner seat/);
      if (calls === 2) assert.match(body.messages.at(-1).content, /Emit only tool_calls/);
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', content: null, tool_calls: [{ id: `bad-${calls}`, type: 'function', function: {
          name: 'write_file', arguments: 'garbage',
        } }],
      } }] });
    },
    runTestCommand: () => assert.fail('Failed planning cannot run tests'),
    publisher: () => assert.fail('Failed planning cannot publish'),
  });
  assert.equal(calls, 2);
  assert.equal(result.failed, true);
  assert.equal(result.planningOnly, true);
  assert.equal(result.result, undefined);
  assert.equal(result.review, undefined);
  assert.deepEqual(result.runs, { planner: result.runs.planner, coder: null, reviewer: null });
  assert.equal(result.command, null);
  assert.match(readFileSync(result.taskPath, 'utf8'), /## Planning failure[\s\S]*after one retry/);
  assert.deepEqual(parseRecipe(readFileSync(result.recipePath, 'utf8')).seats.map(({ id }) => id),
    ['planner', 'coder', 'reviewer']);
  assert.equal(readFileSync(path.join(result.worktreePath, 'README.md'), 'utf8'), originalReadme);
  assert.match(logs.join('\n'), /Planning failed:[\s\S]*coder, reviewer, tests, and publication did not run/);
});

test('an issue-176-shaped planner response reaches coder, real tests, and reviewer without replanning', async (context) => {
  const options = fixture(context);
  options.issue.title = 'Add a smoke regression test';
  options.issue.body = 'Add one passing regression in smoke.test.mjs. Do not change other files.';
  const content = "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    "test('smoke regression', () => assert.equal(2 + 2, 4));\n";
  let plannerCalls = 0;
  let coderCalls = 0;
  let reviewCalls = 0;
  let testCalls = 0;
  const result = await runIssueWithSeats(42, {
    ...options, config: { ...llmConfig, planner: { turn_budget: 1 } }, log: () => {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      const system = body.messages[0].content;
      if (system.startsWith('You are the builtin planner seat.')) {
        plannerCalls += 1;
        return Response.json({ model: 'served-planner', choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', content: 'Plan written.\n```json\n' + JSON.stringify({
            task: options.issue.title, files_allowed: ['smoke.test.mjs'],
            task_class: 'test', difficulty: 2, estimate_min: 45,
            steps: ['Add a deterministic regression', 'Run node --test'],
            acceptance_checks: ['node --test exits 0', 'One passing regression is added'],
            notes: 'No network or wall-clock dependency.',
          }) + '\n```',
          tool_calls: [{ id: 'draft', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: '# Draft\n' }),
          } }],
        } }] });
      }
      if (system.startsWith('You are the builtin reviewer seat.')) {
        reviewCalls += 1;
        assert.equal(testCalls, 1);
        assert.match(body.messages[1].content, /smoke\.test\.mjs/);
        return Response.json({ model: 'served-reviewer', choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: passingReview(body, {
            reasons: ['The scoped regression passed node --test.'], security_notes: ['No protected files changed.'] }),
        } }] });
      }
      coderCalls += 1;
      assert.match(system, /smoke\.test\.mjs/);
      return Response.json({ model: 'served-coder', choices: [{ finish_reason: coderCalls === 1 ? 'tool_calls' : 'stop',
        message: coderCalls === 1 ? { role: 'assistant', tool_calls: [{ id: 'read', type: 'function', function: {
          name: 'read_file', arguments: JSON.stringify({ path: 'smoke.test.mjs' }) } }, { id: 'regression', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'smoke.test.mjs', content }),
        } }] } : { role: 'assistant', content: 'Added the scoped smoke regression.' },
      }] });
    },
    runTestCommand: async (program, args, commandOptions) => {
      testCalls += 1;
      assert.equal(program, process.execPath);
      assert.equal(args[0], '--test');
      // Final verification runs the whole suite of the fixture worktree.
      assert.equal(args.at(-1), '--test-timeout=120000');
      return { stdout: execFileSync(program, args, { ...commandOptions, encoding: 'utf8' }), stderr: '' };
    },
  });
  assert.equal(plannerCalls, 1);
  assert.equal(coderCalls, 2);
  assert.equal(result.failed, false);
  assert.equal(result.result.excellence.pass, true, JSON.stringify(result.result.excellence.reasons));
  assert.equal(result.review.verdict, 'pass', JSON.stringify(result.review.reasons));
  assert.equal(reviewCalls, 1);
  assert.equal(result.runs.planner.metrics.model, 'served-planner');
  assert.equal(result.runs.coder.metrics.model, 'served-coder');
  assert.equal(result.runs.reviewer.metrics.model, 'served-reviewer');
  assert.equal(readFileSync(path.join(result.worktreePath, 'smoke.test.mjs'), 'utf8'), content);
  assert.equal(git(result.worktreePath, 'diff', '--name-only'), 'smoke.test.mjs');
});

test('a configured rerun recovers from previous planner failure in the same issue worktree', async (context) => {
  const options = fixture(context);
  const failed = await runBuiltinIssue(42, {
    ...options, config: llmConfig, log: () => {},
    fetchImpl: async () => Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
      role: 'assistant', content: null, tool_calls: [{ function: { name: 'write_file', arguments: 'garbage' } }],
    } }] }),
  });
  assert.equal(failed.failed, true);
  let coderTurns = 0;
  const result = await runBuiltinIssue(42, {
    ...options, config: llmConfig, log: () => {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      if (body.messages[0].content.startsWith('You are the builtin planner seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({
            title: options.issue.title,
            acceptance_checks: ['The requested behavior in the Ask is implemented'],
            files_allowed: ['README.md'],
          }),
        } }] });
      }
      coderTurns += 1;
      return Response.json({ choices: [coderTurns === 1 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'readme', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
        } }],
      } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Added Status.' } }] });
    },
    runTestCommand: () => assert.fail('Documentation-only changes do not run node --test'),
  });
  assert.equal(result.reused, true);
  assert.equal(result.failed, false);
  assert.equal(result.worktreePath, failed.worktreePath);
  assert.equal(result.result.excellence.pass, true);
  assert.equal(result.review.verdict, 'pass');
  assert.match(readFileSync(path.join(result.archivePath, 'TASK.md'), 'utf8'), /Planning failure/);
  assert.match(readFileSync(path.join(result.worktreePath, 'README.md'), 'utf8'), /## Status\nReady/);
});

test('a rerun never archives or overwrites tracked run artifacts', async (context) => {
  const options = fixture(context);
  const initial = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  const before = readFileSync(initial.taskPath, 'utf8');
  git(initial.worktreePath, 'add', '--', 'RESULT.md');
  await assert.rejects(runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} }),
    /Refusing to replace tracked planning\/run artifacts/);
  assert.equal(readFileSync(initial.taskPath, 'utf8'), before);
});
