import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, writeFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { runBuiltinIssue, fixture, git, llmConfig, stubConfig } from './helpers/builtin.mjs';
import { passingReview } from './helpers/review.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { renderPlan, validatePlan } from '../src/planner/plan.mjs';
import { acquireRepoLock } from '../src/lib/repo-locks.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';

function featureFixture(t) {
  const options = fixture(t);
  options.issue.title = 'Add a profile feature';
  options.issue.body = 'Add a profile feature with documentation and verification.\n\n## Outcomes\n' +
    '- Document a Profile section\n- Verify the Profile section\n\n## Files allowed\n- `README.md`\n';
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  git(options.target, 'worktree', 'add', '-b', 'issue-42', worktree);
  const plan = validatePlan({ outcomes: ['Document profiles', 'Verify profiles'], issues: [100, 101].map((number) => ({
    title: `Document Profile ${number}`, outcome: 'Add a Profile section to README',
    wave: 1, acceptance_checks: ['README has a Profile section'], files_allowed: ['README.md'],
  })) }, { kind: 'feature', filesAllowed: ['README.md'] });
  writeFileSync(path.join(worktree, 'PLAN.md'), renderPlan(plan, { ask: options.issue.body,
    kind: 'feature', title: options.issue.title, reference: 'issue:42' }));
  const children = [];
  const command = async (program, args, cwd) => {
    if (program === 'git') return git(cwd, ...args);
    if (args[0] === 'issue' && args[1] === 'view') {
      return JSON.stringify(Number(args[2]) === 42 ? options.issue : children.find((item) => item.number === Number(args[2])));
    }
    if (args[0] === 'issue' && args[1] === 'list') return JSON.stringify(children);
    if (args[0] === 'label' && args[1] === 'list') return '[{"name":"wave:1"}]';
    if (args[0] === 'pr') return '[]';
    if (args[0] === 'issue' && args[1] === 'create') {
      const number = 100 + children.length;
      const issue = { number, state: 'OPEN', title: args[args.indexOf('--title') + 1],
        body: args[args.indexOf('--body') + 1], labels: [{ name: 'wave:1' }],
        url: `https://github.com/example/project/issues/${number}` };
      children.push(issue);
      return issue.url;
    }
    throw new Error(`Unexpected fake command ${args.join(' ')}`);
  };
  return { ...options, children, runCommand: command };
}

test('parallel feature runs real coder seats in separate Git worktrees with overlapping intervals', { timeout: 30_000 }, async (t) => {
  const options = featureFixture(t);
  mkdirSync(path.join(options.target, '.roster'), { recursive: true });
  writeFileSync(path.join(options.target, '.roster', 'fleet.yml'), formatFleet({ profiles: [1, 2].map((index) => ({
    id: `parallel-fixture-${index}`, base_url: `http://fixture-${index}.invalid/v1`, model: 'local-model', provider: 'vllm',
    context_max: 65536, concurrency: 1, hardware: 'test-only', task_class: ['feat', 'docs'], notes: '',
  })) }));
  const starts = new Map();
  const ends = new Map();
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const logs = [];
  const result = await runBuiltinIssue(42, { ...options, config: llmConfig,
    autoModel: true, metricsLoader: () => [], parallel: 5, log: (text) => logs.push(text),
    onRunEvent(event) {
      if (event.seat === 'coder' && event.type === 'seat-start') {
        starts.set(event.issue, performance.now());
        if (starts.size === 2) release();
      }
      if (event.seat === 'coder' && event.type === 'seat-end') ends.set(event.issue, performance.now());
    },
    fetchImpl: async (_url, request) => {
      assert.match(String(_url), /^http:\/\/fixture-[12]\.invalid\/v1\//);
      const body = JSON.parse(request.body);
      const system = body.messages[0].content;
      if (system.startsWith('You are the builtin reviewer seat.')) {
        return Response.json({ choices: [{ message: { role: 'assistant', content: passingReview(body) }, finish_reason: 'stop' }] });
      }
      if (system.startsWith('You are the builtin planner seat.')) {
        const issue = options.children.find((item) => body.messages[1].content.includes(item.title));
        assert.ok(issue, 'Expected child Ask in planner input');
        const draft = planStub(issue.body, { reference: `issue:${issue.number}`, title: issue.title });
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{
          id: 'task', type: 'function', function: { name: 'write_file',
            arguments: JSON.stringify({ path: 'TASK.md', content: draft.task }) },
        }] } }] });
      }
      await gate;
      return body.messages.some((message) => message.role === 'tool')
        ? Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Profile documented.' } }] })
        : Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{
          id: 'profile', type: 'function', function: { name: 'write_file',
            arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Profile\n\nProfiles work.\n' }) },
        }] } }] });
    },
    runTestCommand: () => assert.fail('Docs-only children must not run tests'),
  });
  assert.equal(result.parallelRun, true);
  assert.equal(result.parallel, 2);
  assert.equal(result.failed, false, JSON.stringify(result.children.map((child) => ({
    issue: child.issue, error: child.error?.message, planner: child.result?.planner?.error,
    review: child.result?.review?.reasons,
  }))));
  assert.equal(result.children.length, 2);
  assert.equal(starts.size, 2);
  assert.ok(Math.max(...starts.values()) < Math.min(...ends.values()), 'Both coders must start before either ends');
  const paths = result.children.map((child) => child.result.worktreePath);
  assert.equal(new Set(paths).size, 2);
  for (const child of result.children) {
    assert.equal(child.result.review.verdict, 'pass');
    assert.equal(git(child.result.worktreePath, 'branch', '--show-current'), `issue-${child.issue}`);
    assert.match(readFileSync(path.join(child.result.worktreePath, 'README.md'), 'utf8'), /## Profile/);
    assert.ok(logs.some((text) => text.startsWith(`[issue-${child.issue}] `)));
  }
  assert.deepEqual(readdirSync(path.join(options.target, '.git', 'roster', 'wave-runs', 'locks')), []);
});

test('parallel one retains the single-child return shape and ordering', async (t) => {
  const options = featureFixture(t);
  const result = await runBuiltinIssue(42, { ...options, config: stubConfig, parallel: 1, log: () => {} });
  assert.equal(result.issue.number, 100);
  assert.equal(result.parent.issue, 42);
  assert.equal(result.parallelRun, undefined);
});

test('a child crash releases its claim and retains the surviving real pipeline result', async (t) => {
  const options = featureFixture(t);
  const failure = new Error('fixture child crashed');
  const command = options.runCommand;
  const result = await runBuiltinIssue(42, { ...options, parallel: 2,
    config: { ...stubConfig, llm: { ...stubConfig.llm, concurrency: 2 } }, log: () => {},
    runCommand: async (program, args, cwd) => {
      if (program === 'gh' && args[0] === 'issue' && args[1] === 'view' && args[2] === '100') throw failure;
      return command(program, args, cwd);
    },
  });
  assert.equal(result.failed, true);
  assert.equal(result.children[0].status, 'rejected');
  assert.equal(result.children[1].status, 'fulfilled');
  assert.equal(result.children[1].result.issue.number, 101);
  const lockRoot = path.join(options.target, '.git', 'roster', 'wave-runs');
  assert.deepEqual(readdirSync(path.join(lockRoot, 'locks')), []);
  const retry = await acquireRepoLock('issue-100', { lockRoot });
  await retry.release();
});

test('a second invocation cannot write an already claimed issue worktree', async (t) => {
  const options = fixture(t);
  const lockRoot = path.join(options.target, '.git', 'roster', 'wave-runs');
  const lock = await acquireRepoLock('issue-42', { lockRoot });
  try {
    await assert.rejects(runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} }),
      (error) => error.code === 'E_LOCK_HELD');
    assert.equal(options.calls.some(({ program }) => program === 'gh'), false);
  } finally {
    await lock.release();
  }
});

test('invalid parallel options fail before GitHub lookup or a shared steering loop', async (t) => {
  const options = fixture(t);
  for (const extra of [{ parallel: 0 }, { parallel: 2, confirm: true },
    { parallel: 2, planMode: true }, { parallel: 2, steeringControl: {} }]) {
    await assert.rejects(runBuiltinIssue(42, { ...options, ...extra, config: stubConfig,
      runCommand: () => assert.fail('Invalid options must fail before side effects') }));
  }
});
