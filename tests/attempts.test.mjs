import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { attemptGates, attemptLimit, runPlanAttempts, selectAttempt } from '../src/lib/attempts.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { loadLearning, validateAttemptEvidence } from '../src/lib/learn.mjs';
import { buildRun } from '../src/metrics/run.mjs';
import { prepareBuiltinPublication } from '../src/lib/builtin.mjs';
import { readLocalRun } from '../src/lib/local-runs.mjs';
import { readStatus, formatStatus } from '../src/lib/status.mjs';
import { listIssueWorktrees } from '../src/lib/worktrees.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { fixture, git, llmConfig, runBuiltinIssue, stubConfig } from './helpers/builtin.mjs';

const profiles = [1, 2].map((index) => ({
  id: `attempt-fixture-${index}`, base_url: `http://attempt-${index}.invalid/v1`, model: `fixture-model-${index}`,
  provider: 'vllm', context_max: 65536, concurrency: 1, hardware: `fake-gpu-${index}`,
  task_class: ['docs', 'feat'], notes: '',
}));

test('attempt limits and strict journal evidence are bounded', () => {
  assert.equal(attemptLimit(), 1);
  assert.equal(attemptLimit(3), 3);
  for (const value of [0, -1, 4, 1.5, '2', NaN]) assert.throws(() => attemptLimit(value), /attempts/);
  assert.throws(() => attemptLimit(2, 17), /maximum/);
  assert.throws(() => validateAttemptEvidence({ count: 2 }), /attempt evidence/);
  const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
  assert.equal(parseConfig(example.replace(/^[ \t]+max_attempts:.*\r?\n/m, '')).seat.max_attempts, 3);
  assert.equal(parseConfig(example.replace('max_attempts: 3', 'max_attempts: 16')).seat.max_attempts, 16);
  for (const value of [0, 17, 1.5]) {
    assert.throws(() => parseConfig(example.replace('max_attempts: 3', `max_attempts: ${value}`)), /max_attempts/);
  }
});

test('selection requires all gates and review, then prefers changed lines, duration and index', () => {
  const candidate = (index, lines, duration, overrides = {}) => ({
    completed: { result: { mode: 'llm' } },
    evidence: { index, changed_lines: lines, duration_ms: duration,
      gates: attemptGates({ tests: { exit_code: 0 }, excellence: { pass: true } }), review: 'pass', ...overrides },
  });
  const smaller = candidate(2, 4, 50);
  const faster = candidate(3, 4, 20);
  const earliest = candidate(1, 4, 20);
  assert.equal(selectAttempt([candidate(4, 1, 1, { review: 'fail' }), candidate(5, 1, 1, {
    gates: { ...earliest.evidence.gates, red_green: 'fail' },
  }), candidate(6, 100, 1), smaller, faster, earliest]), earliest);
  assert.equal(selectAttempt([candidate(1, 0, 0, { review: 'escalate' })]), null);
  assert.equal(attemptGates({ redGreen: { status: 'unavailable' } }).red_green, 'fail');
  assert.equal(attemptGates({ shadow: { status: 'unavailable' } }).shadow, 'fail');
});

async function preparedFixture(t) {
  const options = fixture(t);
  const prepared = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  return { options, prepared };
}

test('two isolated candidates share TASK; failed red/green loser is recorded and only winner publishes', async (t) => {
  const { options, prepared } = await preparedFixture(t);
  const tasks = [];
  let published = 0;
  const completed = await runPlanAttempts({ count: 2, prepared, planner: prepared.planner,
    config: llmConfig, env: options.env, command: options.runCommand, log: () => {},
    choose: async (excluded) => ({ profile: profiles.find(({ id }) => !excluded.includes(id)) }),
    execute: async (ready, planner, selected) => {
      tasks.push(readFileSync(planner.taskPath, 'utf8'));
      assert.equal(git(ready.worktreePath, 'rev-parse', 'HEAD'), git(prepared.worktreePath, 'rev-parse', 'HEAD'));
      assert.equal(readFileSync(path.join(ready.worktreePath, 'README.md'), 'utf8').replaceAll('\r\n', '\n'), '# Example\n');
      writeFileSync(path.join(ready.worktreePath, 'README.md'), `# Example\n\n## Status\n${selected.profile.id}\n`);
      const run = buildRun({ config: { ...llmConfig, llm: { ...llmConfig.llm, model: selected.profile.model } },
        response: { model: selected.profile.model, usage: { prompt_tokens: 5, completion_tokens: 3 } },
        session: ready.session, task: ready.task, env: {} });
      return { ...ready, planner, run, runs: { coder: run }, review: { verdict: 'pass' },
        result: { mode: 'llm', run, tests: { exit_code: 0 }, excellence: { pass: true },
          redGreen: { status: 'checked', notRed: selected.profile.id === profiles[0].id ? ['not red'] : [] } } };
    },
    publish: async (winner) => {
      published += 1;
      assert.equal(winner.winner, 2);
      assert.equal(winner.run.metrics.model, profiles[1].model);
      assert.ok(!existsSync(path.join(options.target, '.worktrees', 'issue-42-a1')));
    },
  });
  assert.equal(published, 1);
  assert.deepEqual(tasks, [prepared.planner.task, prepared.planner.task]);
  assert.equal(completed.attempts, 2);
  assert.equal(completed.run.attempts, 2);
  assert.equal(completed.run.losers[0].gates.red_green, 'fail');
  const records = loadLearning({ cwd: options.target }).runs.filter(({ attempt }) => attempt);
  assert.equal(records.length, 2);
  assert.equal(records[0].attempt.gates.red_green, 'fail');
  assert.equal(records[0].attempt.hardware, 'fake-gpu-1');
  assert.equal(records[1].attempt.winner, 2);
  assert.equal(records[0].model, profiles[0].model);
  assert.equal(records[0].prompt_tokens, 5);
  assert.equal(git(options.target, 'branch', '--list', 'issue-42-a1'), '');
});

test('all failing reviews use failure result and retain only the last diagnostic tree', async (t) => {
  const { options, prepared } = await preparedFixture(t);
  const completed = await runPlanAttempts({ count: 2, prepared, planner: prepared.planner,
    config: llmConfig, env: options.env, command: options.runCommand, log: () => {},
    choose: async (excluded) => ({ profile: profiles.find(({ id }) => !excluded.includes(id)) }),
    execute: async (ready, planner) => ({ ...ready, planner, run: null, result: { mode: 'llm',
      testsSkipped: true, excellence: { pass: true } }, review: { verdict: 'fail' } }),
    publish: () => assert.fail('No failing candidate may publish'),
  });
  assert.equal(completed.failed, true);
  assert.equal(completed.winner, null);
  assert.ok(existsSync(path.join(options.target, '.worktrees', 'issue-42-a2')));
  assert.ok(!existsSync(path.join(options.target, '.worktrees', 'issue-42-a1')));
  assert.equal(loadLearning({ cwd: options.target }).runs.filter(({ attempt }) => attempt).length, 2);
  await assert.rejects(prepareBuiltinPublication(completed, { cwd: options.target,
    config: llmConfig, env: options.env, skipReview: true }), /winning attempt/);
});

test('best-of-N runs real configured seats and one shared planner with fake fleet transports', async (t) => {
  const options = fixture(t);
  mkdirSync(path.join(options.target, '.roster'), { recursive: true });
  writeFileSync(path.join(options.target, '.roster', 'fleet.yml'), formatFleet({ profiles }));
  let plans = 0;
  let published = 0;
  const result = await runBuiltinIssue(42, { ...options, attempts: 2, autoModel: true, config: llmConfig,
    publish: true, env: { ...options.env, GITHUB_APP_ID: '123',
      GITHUB_APP_PRIVATE_KEY_PATH: path.join(options.base, 'fixture.pem') },
    metricsLoader: () => [], log: () => {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      assert.ok(profiles.some(({ model }) => model === body.model));
      if (body.messages[0].content.startsWith('You are the builtin planner seat.')) {
        plans += 1;
        return Response.json({ model: body.model, choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({ title: options.issue.title,
            acceptance_checks: ['README has a Status section'], files_allowed: ['README.md'] }),
        } }] });
      }
      if (body.messages.some(({ role }) => role === 'tool')) return Response.json({
        model: body.model, choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Documented Status.' } }],
      });
      const text = body.model === profiles[0].model ? 'Ready with a longer explanation.\nExtra line.\n' : 'Ready.\n';
      return Response.json({ model: body.model, choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'write', type: 'function', function: { name: 'write_file',
          arguments: JSON.stringify({ path: 'README.md', content: `# Example\n\n## Status\n${text}` }) } }],
      } }] });
    },
    runTestCommand: () => assert.fail('Docs must not run tests'),
    publisher: async (_program, _args, publication) => {
      published += 1;
      assert.equal(path.basename(publication.cwd), 'issue-42-a2');
      assert.equal(publication.env.AI_MODEL, profiles[1].model);
      assert.equal(git(publication.cwd, 'diff', '--cached', '--name-only'), 'README.md');
      return { stdout: 'Merged PR #7 with a merge commit, removed its branch.\n' };
    },
    issueCommenter: async ({ model }) => assert.equal(model, profiles[1].model),
  });
  assert.equal(plans, 1);
  assert.equal(published, 1);
  assert.equal(result.winner, 2);
  assert.equal(result.failed, false);
  assert.equal(result.review.verdict, 'pass');
  assert.equal(path.basename(result.worktreePath), 'issue-42-a2');
  assert.equal(result.attemptResults.length, 2);
  assert.ok(!existsSync(path.join(options.target, '.worktrees', 'issue-42-a1')));
  assert.equal(readFileSync(path.join(options.target, '.worktrees', 'issue-42', 'README.md'), 'utf8').replaceAll('\r\n', '\n'), '# Example\n');
  const offline = await readStatus({ issue: 42, offline: true, cwd: options.target, repoRoot: options.target,
    config: llmConfig, env: options.env });
  assert.equal(offline.branch, 'issue-42-a2');
  assert.match(formatStatus(offline), /Attempt: 2\/2 winner=2/);
  const resumed = await readLocalRun({ number: 42, cwd: options.target, root: options.target,
    config: llmConfig, env: options.env });
  assert.equal(resumed.worktreePath, result.worktreePath);
  assert.equal(resumed.session, result.session);
  const worktrees = await listIssueWorktrees({ cwd: options.target, env: options.env });
  assert.ok(worktrees.some(({ branch }) => branch === 'issue-42-a2'));
});

test('preflight refuses too few profiles and dirty baselines without starting a coder', async (t) => {
  const { options, prepared } = await preparedFixture(t);
  const inputs = { count: 2, prepared, planner: prepared.planner, config: llmConfig,
    env: options.env, command: options.runCommand, log: () => {},
    execute: () => assert.fail('preflight must stop before any coder'),
    choose: async (excluded) => excluded.length ? null : { profile: profiles[0] } };
  await assert.rejects(runPlanAttempts(inputs), /distinct eligible/);
  assert.ok(!existsSync(path.join(options.target, '.worktrees', 'issue-42-a1')));
  writeFileSync(path.join(prepared.worktreePath, 'README.md'), '# Existing user edits\n');
  await assert.rejects(runPlanAttempts(inputs), /clean application baseline/);
  assert.equal(readFileSync(path.join(prepared.worktreePath, 'README.md'), 'utf8'), '# Existing user edits\n');
});

test('existing attempt branches are never adopted, overwritten or removed', async (t) => {
  const { options, prepared } = await preparedFixture(t);
  git(options.target, 'branch', 'issue-42-a1');
  await assert.rejects(runPlanAttempts({ count: 2, prepared, planner: prepared.planner,
    config: llmConfig, env: options.env, command: options.runCommand, log: () => {},
    choose: async (excluded) => ({ profile: profiles.find(({ id }) => !excluded.includes(id)) }),
    execute: () => assert.fail('colliding branches must not run a coder'),
  }), /already exists/);
  assert.ok(git(options.target, 'branch', '--list', 'issue-42-a1'));
  git(options.target, 'branch', '-D', 'issue-42-a1');
  const existing = path.join(options.target, '.worktrees', 'issue-42-a1');
  mkdirSync(existing);
  await assert.rejects(runPlanAttempts({ count: 2, prepared, planner: prepared.planner,
    config: llmConfig, env: options.env, command: options.runCommand, log: () => {},
    choose: async (excluded) => ({ profile: profiles.find(({ id }) => !excluded.includes(id)) }),
    execute: () => assert.fail('pre-existing empty directories must not be adopted'),
  }), /path already exists/);
  assert.ok(existsSync(existing));
  assert.equal(git(options.target, 'branch', '--list', 'issue-42-a1'), '');
});

test('coder errors are recorded and all-fail preserves the existing error/result failure shape', async (t) => {
  const { options, prepared } = await preparedFixture(t);
  const errors = [];
  await assert.rejects(runPlanAttempts({ count: 2, prepared, planner: prepared.planner,
    config: llmConfig, env: options.env, command: options.runCommand, log: () => {},
    choose: async (excluded) => ({ profile: profiles.find(({ id }) => !excluded.includes(id)) }),
    execute: async (ready) => {
      const error = new Error('Failed deterministic gate');
      error.result = { mode: 'llm', tests: { exit_code: 1 }, excellence: { pass: false } };
      errors.push(error);
      throw error;
    }, publish: () => assert.fail('failed gate must never publish'),
  }), (error) => {
    assert.equal(error, errors[1]);
    assert.equal(error.attemptResults.length, 2);
    assert.equal(error.attemptResults[0].gates.tests, 'fail');
    return true;
  });

  assert.equal(loadLearning({ cwd: options.target }).runs.filter(({ attempt }) => attempt).length, 2);
  assert.ok(!existsSync(path.join(options.target, '.worktrees', 'issue-42-a1')));
  assert.ok(existsSync(path.join(options.target, '.worktrees', 'issue-42-a2')));
});

test('cancellation records completed evidence, starts no later candidate and cleans only owned losers', async (t) => {
  const { options, prepared } = await preparedFixture(t);
  const controller = new AbortController();
  const candidates = [...profiles, { ...profiles[1], id: 'attempt-fixture-3' }];
  let executions = 0;
  await assert.rejects(runPlanAttempts({ count: 3, prepared, planner: prepared.planner,
    config: llmConfig, env: options.env, signal: controller.signal, command: options.runCommand,
    cleanupCommand: options.runCommand, log: () => {},
    choose: async (excluded) => ({ profile: candidates.find(({ id }) => !excluded.includes(id)) }),
    execute: async (ready) => {
      executions += 1;
      if (executions === 2) controller.abort();
      return { ...ready, run: null, result: { mode: 'llm', testsSkipped: true, excellence: { pass: true } },
        review: { verdict: 'pass' } };
    }, publish: () => assert.fail('cancelled selection must not publish'),
  }));
  assert.equal(executions, 2);
  assert.ok(!existsSync(path.join(options.target, '.worktrees', 'issue-42-a1')));
  assert.ok(existsSync(path.join(options.target, '.worktrees', 'issue-42-a2')));
  assert.ok(!existsSync(path.join(options.target, '.worktrees', 'issue-42-a3')));
  assert.equal(loadLearning({ cwd: options.target }).runs.filter(({ attempt }) => attempt).length, 2);
});

test('attempts 1 keeps ordinary stub result and rejects conflicting opt-ins before issue preparation', async (t) => {
  const options = fixture(t);
  const result = await runBuiltinIssue(42, { ...options, config: stubConfig, attempts: 1, log: () => {} });
  assert.equal(result.attempts, undefined);
  assert.equal(path.basename(result.worktreePath), 'issue-42');
  for (const conflict of [{ attempts: 4 }, { attempts: 2, confirm: true }, { attempts: 2, parallel: 2 },
    { attempts: 2, skipReview: true }, { attempts: 2, planMode: true }]) {
    await assert.rejects(runBuiltinIssue(42, { ...options, ...conflict, config: stubConfig,
      runCommand: () => assert.fail('invalid opt-in must not prepare an issue') }), /attempts|Multiple attempts/);
  }
});
