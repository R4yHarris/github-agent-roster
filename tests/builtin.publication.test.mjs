// Builtin seat orchestration: Publication staging, reviewer gating, keys, memory, and journal defects.
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

test('LLM run stages only allowed code, supplies AI-Run fields, and invokes the SDK only with --publish', async (context) => {
  const options = fixture(context);
  const logs = [];
  let completion = 0;
  let published = 0;
  let commented = 0;
  const fetchImpl = async (url, request) => {
    assert.equal(url, 'http://127.0.0.1:8000/v1/chat/completions');
    completion += 1;
    const body = JSON.parse(request.body);
    assert.equal(body.model, 'local-model');
    assert.equal(request.headers.Authorization, 'Bearer private-key');
    assert.ok(!request.body.includes('private-key'));
    if (completion === 1) {
      assert.deepEqual(body.tools.map(({ function: tool }) => tool.name), ['write_file']);
      return { ok: true, status: 200, json: async () => ({
        choices: [{ message: { role: 'assistant', content: JSON.stringify({
          title: 'Add Status to README',
          acceptance_checks: ['README has a Status section'],
          files_allowed: ['README.md'],
        }) } }],
        model: 'actual-planner-model',
        usage: { prompt_tokens: 5, completion_tokens: 2 },
      }) };
    }
    if (completion === 2) {
      assert.match(body.messages[0].content, /## TASK\.md\n\n# Outcome: Add Status to README/);
      assert.match(readFileSync(path.join(options.target, '.worktrees', 'issue-42', 'ESTIMATE.md'), 'utf8'),
        /model: local-model/);
      return { ok: true, status: 200, json: async () => ({
        choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant',
          tool_calls: [{ id: 'update', type: 'function', function: {
            name: 'write_file',
            arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
          } }],
        } }],
        usage: { prompt_tokens: 10, completion_tokens: 3 },
      }) };
    }
    return { ok: true, status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Updated README; tests pass.' } }],
      model: 'actual-coder-model', usage: { prompt_tokens: 100, completion_tokens: 40 },
    }) };
  };
  const result = await runBuiltinIssue(42, {
    ...options, config: vllmConfig,
    env: { ...options.env, ROSTER_API_KEY: 'private-key', GITHUB_APP_ID: '123',
      AI_MODEL: 'GPT-6.1-Sol', AI_PROVIDER: 'github-copilot', AI_MODEL_VERSION: 'stale',
      AI_CONTEXT_USED: '1000000', AI_CONTEXT_MAX: '1000000', AI_CONTEXT_OUT: '999',
      GITHUB_APP_PRIVATE_KEY_PATH: path.join(options.base, 'app.pem') },
    publish: true, log: (line) => logs.push(line), fetchImpl,
    runTestCommand: () => assert.fail('Documentation-only changes do not run node --test'),
    publisher: async (program, args, publication) => {
      published += 1;
      assert.equal(program, process.execPath);
      assert.deepEqual(args, [
        path.join(options.contracts, 'scripts', 'agent-pr.mjs'),
        '--message', buildPublishMessage({
          subject: 'feat: issue 42', model: 'actual-coder-model',
          summary: 'Updated README; tests pass.', issueNumber: 42,
          seats: 'planner, coder, reviewer (pass)',
          testsSkipped: true,
        }), '--model', 'actual-coder-model', '--merge-when-green',
      ]);
      assert.equal(publication.cwd, path.join(options.target, '.worktrees', 'issue-42'));
      assert.equal(publication.env.ROSTER_API_KEY, undefined);
      assert.equal(publication.env.GITHUB_APP_ID, '123');
      assert.equal(publication.env.AI_MODEL, 'actual-coder-model');
      assert.equal(publication.env.AI_PROVIDER, 'local');
      assert.equal(publication.env.AI_MODEL_VERSION, '-');
      assert.equal(publication.env.AI_EFFORT, 'm');
      assert.equal(publication.env.AI_CONTEXT_USED, '100');
      assert.equal(publication.env.AI_CONTEXT_OUT, '40');
      assert.equal(publication.env.AI_CONTEXT_MAX, undefined);
      assert.equal(publication.env.AI_SESSION, 'roster-42-coder');
      assert.equal(publication.env.AI_TASK, 'issue-42');
      assert.equal(git(publication.cwd, 'diff', '--cached', '--name-only'), 'README.md');
      return { stdout: 'Merged PR #7 with a merge commit, removed its branch.\n' };
    },
    issueCommenter: async ({ issue, pullNumber, model, runLine, run }) => {
      commented += 1;
      assert.equal(issue.number, 42);
      assert.equal(pullNumber, 7);
      assert.equal(model, 'actual-coder-model');
      assert.equal(run.metrics.prompt_tokens, 100);
      assert.equal(run.metrics.completion_tokens, 40);
      assert.equal(runLine, packAgentRun({ AI_PROVIDER: 'local', AI_MODEL: 'actual-coder-model',
        AI_MODEL_VERSION: '-', AI_EFFORT: 'm',
        AI_CONTEXT_USED: '100', AI_CONTEXT_OUT: '40',
        AI_SESSION: 'roster-42-coder', AI_TASK: 'issue-42' }));
    },
  });
  assert.equal(published, 1);
  assert.equal(commented, 1);
  assert.equal(completion, 3);
  assert.match(readFileSync(path.join(result.worktreePath, 'README.md'), 'utf8'), /## Status\nReady/);
  assert.equal(result.result.tests, undefined);
  assert.equal(result.run.line, packAgentRun(result.run.env));
  assert.equal(result.review.verdict, 'pass');
  assert.match(readFileSync(result.review.reviewPath, 'utf8'), /Verdict: pass/);
  assert.equal(result.run.provider, 'vllm');
  assert.equal(result.run, result.result.run);
  assert.deepEqual(result.result.usage, { prompt_tokens: 110, completion_tokens: 43 });
  assert.equal(result.runs.planner.metrics.model, 'actual-planner-model');
  assert.equal(result.runs.coder.metrics.model, 'actual-coder-model');
  assert.equal(result.runs.reviewer.metrics.model, 'local-model');
  assert.match(result.runs.planner.line, /\|5\/-\|2\|roster-42-planner\|issue-42$/);
  assert.match(result.runs.coder.line, /\|100\/-\|40\|roster-42-coder\|issue-42$/);
  assert.match(result.runs.reviewer.line, /\|4\/-\|2\|roster-42-reviewer\|issue-42$/);
  const seatRecords = loadLearning({ cwd: options.target }).runs;
  assert.deepEqual(seatRecords.map(({ provider }) => provider), ['vllm', 'vllm', 'vllm']);
  assert.deepEqual(seatRecords.slice(0, 2).map(({ model, effort, context_used, context_out }) =>
    ({ model, effort, context_used, context_out })), [
    { model: 'actual-planner-model', effort: 'm', context_used: 5, context_out: 2 },
    { model: 'actual-coder-model', effort: 'm', context_used: 100, context_out: 40 },
  ]);
  for (const [index, seat] of ['planner', 'coder', 'reviewer'].entries()) {
    for (const [field, value] of Object.entries(result.runs[seat].metrics)) {
      assert.equal(seatRecords[index][field], value);
    }
  }
  assert.ok(seatRecords.every(({ context_max }) => context_max === undefined));
  assert.deepEqual(seatRecords.map(({ excellence }) => excellence), [undefined, 'pass', undefined]);
  assert.deepEqual(seatRecords[1].defects, []);
  assert.equal((logs.join('\n').match(/AI-Run:/g) ?? []).length, 3);
  assert.match(logs.join('\n'), /AI_CONTEXT_MAX=\n/);
  assert.ok(logs.some((line) => line.includes('Merged PR #7')));
  assert.ok(!logs.join('\n').includes('private-key'));
  assert.match(logs.join('\n'), /Reviewed worktree:[\s\S]*git diff --stat:[\s\S]*README.md/);
  assert.match(logs.join('\n'), /roster eval roster-42-coder accept 1 n --minutes M/);
});

test('a failed reviewer keeps coder changes but blocks publication unless explicitly bypassed', async (context) => {
  for (const [skipReview, reviewRequired] of [[false, true], [true, true], [false, false]]) {
    const options = multiFileFixture(context);
    const logs = [];
    let coderTurns = 0;
    let published = 0;
    const fetchImpl = async (_url, request) => {
      const body = JSON.parse(request.body);
      const system = body.messages[0].content;
      if (system.startsWith('You are the builtin research step.')) {
        return { status: 200, json: async () => ({
          choices: [{ finish_reason: 'stop',
            message: { role: 'assistant', content: 'Read-only inventory.' } }],
        }) };
      }
      if (system.startsWith('You are the builtin reviewer seat.')) {
        assert.equal(body.tools, undefined);
        assert.match(body.messages[1].content, /README has a Status section/);
        assert.match(body.messages[1].content, /\+## Status/);
        assert.match(body.messages[1].content, /Checks: PASS/);
        return { status: 200, json: async () => ({
          choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
            verdict: 'fail', reasons: ['The diff lacks sufficient evidence for a full review.'],
            security_notes: ['Inspect downstream use of the edited section.'],
          }) } }],
        }) };
      }
      if (system.startsWith('You are the builtin planner seat.')) {
        return { status: 200, json: async () => ({
          choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
            title: 'Add status', acceptance_checks: ['node --test exits 0', 'README has a Status section'],
            files_allowed: multiFileScope,
          }) } }],
        }) };
      }
      coderTurns += 1;
      return { status: 200, json: async () => ({
        choices: [coderTurns === 1 ? { finish_reason: 'tool_calls',
          message: { role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
            name: 'write_file',
            arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
          } }] } } : { finish_reason: 'stop',
          message: { role: 'assistant', content: 'Added the Status section; tests pass.' } }],
      }) };
    };
    const args = {
      ...options, config: { ...llmConfig, review: { required: reviewRequired } },
      publish: true, skipReview, fetchImpl, log: (message) => logs.push(message),
      env: { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
      runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
      publisher: async (_program, params, publication) => {
        published += 1;
        assert.ok(params[2].includes(`## Seats\n\nplanner, coder, reviewer (${skipReview
          ? 'gate bypassed with --skip-review' : 'gate not required by configuration'})`));
        assert.equal(git(publication.cwd, 'diff', '--cached', '--name-only'), 'README.md');
        return { stdout: 'Merged PR #7 with a merge commit, removed its branch.\n' };
      },
      issueCommenter: async ({ model }) => { assert.equal(model, 'local-model'); },
    };
    if (skipReview || !reviewRequired) {
      const run = await runIssueWithSeats(42, args);
      assert.equal(run.review.verdict, 'fail');
      assert.match(run.command, /--model local-model --merge-when-green/);
    } else {
      await assert.rejects(runIssueWithSeats(42, args), /passing REVIEW\.md/);
    }
    assert.equal(published, skipReview || !reviewRequired ? 1 : 0);
    assert.match(logs.join('\n'), /Review failure: The diff lacks sufficient evidence/);
    if (skipReview || !reviewRequired) {
      assert.match(logs.join('\n'), /WARNING: review failed; publication is permitted only because[\s\S]*These changes are not approved/);
    } else {
      assert.doesNotMatch(logs.join('\n'), /WARNING: review failed; publication is permitted/);
    }
    const worktree = path.join(options.target, '.worktrees', 'issue-42');
    assert.match(readFileSync(path.join(worktree, 'README.md'), 'utf8'), /## Status/);
    assert.match(readFileSync(path.join(worktree, 'RESULT.md'), 'utf8'), /Checks: PASS/);
    assert.match(readFileSync(path.join(worktree, 'REVIEW.md'), 'utf8'),
      /Verdict: fail[\s\S]*## Security notes/);
    if (!skipReview && reviewRequired) assert.equal(git(worktree, 'diff', '--cached', '--name-only'), '');
  }
});

test('publication refuses a REVIEW.md changed after a passing reviewer without staging code', async (context) => {
  const options = fixture(context);
  let turns = 0;
  const run = await runBuiltinIssue(42, {
    ...options, config: llmConfig, log: () => {},
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    fetchImpl: async (_url, request) => {
      turns += 1;
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: turns === 2 ? 'tool_calls' : 'stop', message: {
          role: 'assistant',
          content: turns === 1 ? JSON.stringify({
            title: 'Add status',
            acceptance_checks: ['The requested behavior in the Ask is implemented'],
            files_allowed: ['README.md'],
          }) : turns === 2 ? null : 'README updated.',
          ...(turns === 2 ? { tool_calls: [{ id: 'write', type: 'function',
            function: { name: 'write_file', arguments: JSON.stringify({
              path: 'README.md', content: '# Example\n\n## Status\nReady.\n',
            }) } }] } : {}),
        } }],
      }) };
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.equal(run.review.verdict, 'pass');
  writeFileSync(run.review.reviewPath, run.review.content.replace('Verdict: pass', 'Verdict: fail'));
  await assert.rejects(prepareBuiltinPublication(run, {
    config: llmConfig, cwd: options.cwd,
    env: { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
  }), /REVIEW\.md changed after review/);
  assert.equal(git(run.worktreePath, 'diff', '--cached', '--name-only'), '');
});

test('a merged PR still receives an issue comment when publisher local cleanup fails', async (context) => {
  const options = fixture(context);
  let turns = 0;
  let commented = false;
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, publish: true, log: () => {},
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key',
      GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
    fetchImpl: async () => {
      turns += 1;
      if (turns === 1) return { status: 200, json: async () => ({ choices: [{
        message: { role: 'assistant', content: JSON.stringify({
          title: 'Add status',
          acceptance_checks: ['The requested behavior in the Ask is implemented'],
          files_allowed: ['README.md'],
        }) },
      }] }) };
      if (turns === 2) return { status: 200, json: async () => ({ choices: [{
        finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{
          id: 'write', type: 'function', function: { name: 'write_file',
            arguments: JSON.stringify({ path: 'README.md', content: '# Updated\n' }) },
        }] },
      }] }) };
      return { status: 200, json: async () => ({ choices: [{
        finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' },
      }] }) };
    },
    runTestCommand: () => assert.fail('Documentation-only changes do not run node --test'),
    publisher: async () => { throw Object.assign(new Error('publisher failed'), {
      stderr: 'PR #7 was merged; local cleanup is incomplete.',
    }); },
    issueCommenter: async ({ issue, pullNumber, model, runLine }) => {
      assert.equal(issue.number, 42);
      assert.equal(pullNumber, 7);
      assert.equal(model, 'local-model');
      assert.match(runLine, /\|roster-42-coder\|issue-42$/);
      commented = true;
    },
  }), /PR #7 merged and issue commented, but local publisher cleanup failed/);
  assert.equal(turns, 3);
  assert.equal(commented, true);
});

test('default planner/coder run stops after a denied managed-file write', async (context) => {
  const options = fixture(context);
  options.issue.title = 'Implement the app';
  options.issue.body = 'Add src/app.mjs.\n\n## Acceptance checks\n- node --test exits 0\n' +
    '\n## Files allowed\n- `src/app.mjs`\n';
  let completion = 0;
  const worktreePath = path.join(options.target, '.worktrees', 'issue-42');
  let handoff;
  const fetchImpl = async (_url, request) => {
    completion += 1;
    const body = JSON.parse(request.body);
    if (completion === 1) {
      assert.deepEqual(body.tools.map(({ function: tool }) => tool.name), ['write_file']);
      return { ok: true, status: 200, json: async () => ({ choices: [{ message: { role: 'assistant', content: JSON.stringify({
        title: 'Implement the app',
        acceptance_checks: ['node --test exits 0'],
        files_allowed: ['src/app.mjs'],
      }) } }] }) };
    }
    if (completion === 2) {
      handoff = ['RECIPE.yml', 'TASK.md'].map((name) => readFileSync(path.join(worktreePath, name), 'utf8'));
      assert.match(body.messages[0].content, /## Files allowed\n- `src\/app\.mjs`/);
      const write = (id, file, content) => ({
        id, type: 'function',
        function: { name: 'write_file', arguments: JSON.stringify({ path: file, content }) },
      });
      return { ok: true, status: 200, json: async () => ({ choices: [{
        finish_reason: 'tool_calls',
        message: { role: 'assistant', tool_calls: [
          write('recipe', 'RECIPE.yml', 'tampered'),
          write('code', 'src/app.mjs', 'export const ready = true;\n'),
          write('task', 'TASK.md', 'tampered'),
        ] },
      }] }) };
    }
    assert.fail('A policy denial must stop before another model request');
  };
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, fetchImpl, log: () => {},
    vault: { get: async () => undefined },
    runTestCommand: () => assert.fail('Denied writes must stop before tests'),
  }), /Writing RECIPE\.yml is not allowed by TASK\.md or worktree policy/);
  assert.equal(completion, 2);
  assert.deepEqual(parseRecipe(readFileSync(path.join(worktreePath, 'RECIPE.yml'), 'utf8')).seats.map(({ id }) => id),
    ['planner', 'coder', 'reviewer']);
  assert.deepEqual(['RECIPE.yml', 'TASK.md'].map((name) => readFileSync(path.join(worktreePath, name), 'utf8')), handoff);
  assert.equal(existsSync(path.join(worktreePath, 'src', 'app.mjs')), false);
  assert.match(readFileSync(path.join(worktreePath, 'RESULT.md'), 'utf8'), /Checks: FAIL/);
  assert.equal(options.calls.filter(({ program, args }) =>
    program === 'git' && args[0] === 'worktree' && args[1] === 'add').length, 1);
});

test('a tool-writing planner hands validated artifacts to the scoped coder and read-only reviewer', async (context) => {
  const options = fixture(context);
  const draft = planStub(options.issue.body, { reference: 'issue:42', title: options.issue.title });
  let plannerTurns = 0;
  let coderTurns = 0;
  const result = await runBuiltinIssue(42, {
    ...options, config: llmConfig, log: () => {}, env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      if (body.messages[0].content.startsWith('You are the builtin planner seat.')) {
        assert.doesNotMatch(options.stderr, /Writing the plan/);
        assert.doesNotMatch(options.stderr, /http chat|start seat|\d{4}-\d\d-\d\dT/);
        plannerTurns += 1;
        assert.deepEqual(body.tools.map(({ function: tool }) => tool.name), ['write_file']);
        if (plannerTurns === 1) return Response.json({
          choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant',
            tool_calls: [{ id: 'task', type: 'function', function: {
              name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: draft.task }),
            } }],
          } }],
        });
        assert.equal(JSON.parse(body.messages.at(-1).content).path, 'TASK.md');
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: 'The task draft is ready.',
        } }] });
      }
      coderTurns += 1;
      if (coderTurns === 1) return Response.json({
        choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant',
          tool_calls: [{ id: 'readme', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({
              path: 'README.md', content: '# Example\n\n## Status\nReady.\n',
            }),
          } }],
        } }],
      });
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: 'Added the Status section; tests pass.',
      } }] });
    },
    runTestCommand: () => assert.fail('Documentation-only changes do not run node --test'),
  });
  assert.equal(plannerTurns, 1);
  assert.equal(coderTurns, 2);
  assert.equal(result.result.excellence.pass, true);
  assert.equal(result.review.verdict, 'pass');
  const liveLog = readFileSync(result.logPath, 'utf8');
  assert.notEqual(liveLog, options.stderr);
  assert.doesNotMatch(options.stderr, /Reading|Saving README\.md\.|Running tests\./);
  assert.doesNotMatch(options.stderr, /http chat|model=|host=|elapsed_ms=|\d{4}-\d\d-\d\dT/);
  assert.match(liveLog, /seat planner tool write_file path="TASK\.md"/);
  assert.match(liveLog, /seat coder tool write_file path="README\.md"/);
  assert.doesNotMatch(liveLog, /seat coder tool run_test/);
  for (const seat of ['planner', 'coder', 'reviewer']) {
    assert.match(liveLog, new RegExp(`seat ${seat} http chat\\.completions ok status=200`));
    assert.match(liveLog, new RegExp(`seat ${seat} elapsed_ms=\\d+ mode=llm`));
  }
  assert.match(liveLog, /model="local-model" host="localhost:1234"/);
  assert.doesNotMatch(liveLog, /http:\/\/|\/v1|test-only-key|Added the Status section|## Status|# Example|You are the builtin/);
  assert.equal(readFileSync(result.taskPath, 'utf8'), result.planner.task);
  assert.equal(readFileSync(result.recipePath, 'utf8'), result.planner.recipe);
  assert.equal(readFileSync(result.planner.estimatePath, 'utf8'), result.planner.estimate);
  assert.match(readFileSync(path.join(result.worktreePath, 'README.md'), 'utf8'), /## Status\nReady/);
});

test('planner and coder use an environment key before the vault and fall back to the vault', async (context) => {
  for (const source of ['environment', 'vault']) {
    const options = multiFileFixture(context);
    const key = 'test-only-llm-key';
    let vaultReads = 0;
    let requests = 0;
    const vault = { get: async (name) => {
      assert.equal(name, 'ROSTER_API_KEY');
      vaultReads += 1;
      return key;
    } };
    const fetchImpl = async (_url, request) => {
      requests += 1;
      assert.equal(request.headers.Authorization, `Bearer ${key}`);
      if (requests === 1) {
        return { status: 200, json: async () => ({
          choices: [{ message: { role: 'assistant', content: JSON.stringify({
            title: 'Add status',
            acceptance_checks: ['node --test exits 0'],
            files_allowed: multiFileScope,
          }) } }],
          usage: { prompt_tokens: 3, completion_tokens: 2 },
        }) };
      }
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }],
        usage: { prompt_tokens: 5, completion_tokens: 1 },
      }) };
    };
    const logs = [];
    const result = await runBuiltinIssue(42, {
      ...options, config: llmConfig, vault, fetchImpl,
      env: { ...options.env, ROSTER_API_KEY: source === 'environment' ? key : undefined },
      runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
      log: (message) => logs.push(message),
    });
    assert.equal(requests, 2);
    assert.equal(vaultReads, source === 'environment' ? 0 : 2);
    assert.equal(result.runs.planner.env.AI_CONTEXT_USED, '3');
    assert.equal(result.runs.coder.env.AI_CONTEXT_USED, '5');
    assert.ok(!logs.join('\n').includes(key));
    assert.ok(!readFileSync(path.join(options.repoRoot, '.roster', 'memory', 'coder.jsonl'),
      'utf8').includes(key));
  }
});

test('planner reads its last 20 lines; slice coder omits memory input and both append separately', async (context) => {
  const options = multiFileFixture(context);
  const directory = path.join(options.repoRoot, '.roster', 'memory');
  mkdirSync(directory, { recursive: true });
  for (const seat of ['planner', 'coder']) {
    writeFileSync(path.join(directory, `${seat}.jsonl`),
      `${Array.from({ length: 25 }, (_, index) => JSON.stringify({ seat, index })).join('\n')}\n`);
  }
  let calls = 0;
  const fetchImpl = async (_url, request) => {
    calls += 1;
    const body = JSON.parse(request.body);
    const context = body.messages[calls === 1 ? 1 : 0].content;
    if (calls === 1) {
      assert.match(context, /"seat":"planner","index":5/);
      assert.match(context, /"seat":"planner","index":24/);
      assert.doesNotMatch(context, /"seat":"planner","index":4|"seat":"coder"/);
      return { status: 200, json: async () => ({ choices: [{ message: {
        role: 'assistant', content: JSON.stringify({
          title: 'Add status', acceptance_checks: ['node --test exits 0'],
          files_allowed: multiFileScope, task_class: 'feat', difficulty: 4,
        }),
      } }] }) };
    }
    assert.doesNotMatch(context, /"seat":"(?:planner|coder)"|## Seat memory/);
    return { status: 200, json: async () => ({ choices: [{
      finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' },
    }] }) };
  };
  const result = await runBuiltinIssue(42, {
    ...options, config: llmConfig, fetchImpl, log: () => {},
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    runTestCommand: async () => ({ stdout: 'tests pass', stderr: '' }),
  });
  assert.equal(calls, 2);
  for (const seat of ['planner', 'coder']) {
    const records = readFileSync(path.join(directory, `${seat}.jsonl`), 'utf8')
      .trimEnd().split('\n').map((line) => JSON.parse(line));
    assert.equal(records.length, 26);
    assert.deepEqual(records[0], { seat, index: 0 });
    assert.equal(records.at(-1).session, result.sessions[seat]);
    assert.equal(records.at(-1).status, 'llm');
  }
});

test('builtin seats record runs automatically without an AI-Eval', async (context) => {
  const options = fixture(context);
  const result = await runBuiltinIssue(42, {
    ...options, config: stubConfig, log: () => {},
    env: { ...options.env, GITHUB_AGENT_CONTRACTS: resolveContractsPath() },
    fetchImpl: () => { throw new Error('stub must not contact an LLM'); },
  });
  assert.deepEqual(loadLearning({ cwd: options.target }).runs, [
    { session: result.sessions.planner, task: 'issue-42', task_class: 'feat' },
    { session: result.sessions.coder, task: 'issue-42', task_class: 'feat', excellence: 'fail',
      defects: result.result.excellence.reasons },
    { session: result.sessions.reviewer, task: 'issue-42', task_class: 'feat' },
  ]);
  assert.equal(existsSync(path.join(options.target, '.roster', 'evals.jsonl')), false);
});

test('detects a changed recipe after the coder runs tests and refuses publication', async (context) => {
  const options = multiFileFixture(context);
  let published = false;
  const fetchImpl = async (_url, request) => {
    const body = JSON.parse(request.body);
    if (body.messages[0].content.startsWith('You are the builtin planner seat.')) {
      return { ok: true, status: 200, json: async () => ({ choices: [{ message: { role: 'assistant', content: JSON.stringify({
        title: 'Update README', acceptance_checks: ['node --test exits 0'],
        files_allowed: multiFileScope,
      }) } }] }) };
    }
    return { ok: true, status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done' } }],
    }) };
  };
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, publish: true, fetchImpl,
    vault: { get: async () => undefined },
    env: { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
    runTestCommand: async (_program, _args, { cwd }) => {
      writeFileSync(path.join(cwd, 'RECIPE.yml'), 'tampered');
      return { stdout: 'passed', stderr: '' };
    },
    publisher: async () => { published = true; },
  }), /Diff path is protected or outside TASK\.md allowed paths: RECIPE\.yml/);
  assert.equal(published, false);
  assert.equal(existsSync(path.join(options.target, '.worktrees', 'issue-42', 'RESULT.md')), true);
  assert.equal(existsSync(path.join(options.target, '.worktrees', 'issue-42', 'REVIEW.md')), false);
  const records = loadLearning({ cwd: options.target }).runs;
  assert.deepEqual(records.map(({ session, excellence }) => ({ session, excellence })), [
    { session: 'roster-42-planner', excellence: undefined },
    { session: 'roster-42-coder', excellence: 'fail' },
  ]);
  assert.ok(records.slice(0, 2).every(({ model }) => model === 'local-model'));
  assert.ok(records[1].defects.some((reason) => reason.includes('RECIPE.yml')));
  assert.equal(existsSync(path.join(options.target, '.roster', 'evals.jsonl')), false);
});

test('publication preparation requires a model before reading files, staging, or invoking the SDK', async () => {
  await assert.rejects(prepareBuiltinPublication({
    worktreePath: 'missing-worktree', planner: { recipe: '', task: '' },
    runs: { coder: { env: {} } },
    result: { mode: 'llm', tests: { exit_code: 0 }, excellence: { pass: true } },
  }, {
    config: stubConfig, env: { GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
  }), /set model/);
});

test('secret-path touches by the test subprocess are retained as redacted journal defects', async (context) => {
  const options = multiFileFixture(context);
  let requests = 0;
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, log: () => {},
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    fetchImpl: async () => {
      requests += 1;
      return { status: 200, json: async () => ({ choices: [{
        finish_reason: 'stop',
        message: { role: 'assistant', content: requests === 1 ? JSON.stringify({
          title: 'Update README', acceptance_checks: ['node --test exits 0'], files_allowed: multiFileScope,
        }) : 'Reviewed README.',
        },
      }] }) };
    },
    runTestCommand: async (_program, _args, { cwd }) => {
      writeFileSync(path.join(cwd, '.env'), 'TEST_SECRET=test-only-key\n');
      return { stdout: 'passed', stderr: '' };
    },
  }), /Diff path is protected or outside TASK\.md allowed paths: \.env/);
  const records = loadLearning({ cwd: options.target }).runs;
  const coder = records.find(({ session }) => session === 'roster-42-coder');
  assert.equal(coder.excellence, 'fail');
  assert.ok(coder.defects.some((reason) => reason.endsWith(': .env')));
  assert.ok(!JSON.stringify(records).includes('test-only-key'));
  assert.equal(existsSync(path.join(options.target, '.roster', 'evals.jsonl')), false);
});

test('publication rechecks append new secret-path defects after an initially passing run', async (context) => {
  const options = multiFileFixture(context);
  let requests = 0;
  const run = await runBuiltinIssue(42, {
    ...options, config: llmConfig, log: () => {},
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    fetchImpl: async () => {
      requests += 1;
      return { status: 200, json: async () => ({ choices: [{
        finish_reason: 'stop', message: { role: 'assistant', content: requests === 1 ? JSON.stringify({
          title: 'Update README', acceptance_checks: ['node --test exits 0'], files_allowed: multiFileScope,
        }) : 'Reviewed README.' },
      }] }) };
    },
    runTestCommand: async () => ({ stdout: 'passed', stderr: '' }),
  });
  assert.equal(run.result.excellence.pass, true);
  writeFileSync(path.join(run.worktreePath, '.env'), 'TEST_SECRET=test-only-key\n');
  await assert.rejects(prepareBuiltinPublication(run, {
    config: llmConfig, cwd: options.cwd,
    skipReview: true,
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key',
      GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
  }), /Publishing refused by excellence gate/);
  const records = loadLearning({ cwd: options.target }).runs;
  assert.equal(records.length, 4);
  assert.equal(records[1].excellence, 'pass');
  assert.equal(records[3].excellence, 'fail');
  assert.ok(records[3].defects.some((reason) => reason.endsWith(': .env')));
  assert.ok(!JSON.stringify(records).includes('test-only-key'));
});

test('staging refuses changes outside the task scope', async (context) => {
  const options = fixture(context);
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  git(options.target, 'worktree', 'add', '-b', 'issue-42', worktree);
  writeFileSync(path.join(worktree, 'README.md'), '# Out of scope change\n');
  await assert.rejects(stageReviewedFiles(worktree, ['src/**']), /outside TASK.md scope/);
  assert.equal(git(worktree, 'diff', '--cached', '--name-only'), '');
});


for (const [capability, failure] of [
  ['max_difficulty: 1', /max_difficulty 1/],
  ['skills: []', /does not cover task skills/],
]) {
  test(`recipe ${capability} blocks the coder before it starts`, async (context) => {
    const options = fixture(context);
    const first = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
    const recipe = readFileSync(first.recipePath, 'utf8').replace('  - id: coder\n',
      `  - id: coder\n    ${capability}\n`);
    writeFileSync(first.recipePath, recipe);
    await assert.rejects(runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
      onRunEvent(event) { assert.notEqual(event.seat, 'coder'); },
      fetchImpl() { assert.fail('blocked assignment must not contact an LLM'); },
    }), failure);
    assert.equal(existsSync(path.join(first.worktreePath, 'RESULT.md')), false);
    assert.equal(existsSync(path.join(first.worktreePath, 'REVIEW.md')), false);
  });
}
