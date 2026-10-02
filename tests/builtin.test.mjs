import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { writeAsk } from '../src/lib/ask.mjs';
import {
  prepareBuiltinPublication, runBuiltinAsk, runBuiltinIssue as runIssueWithSeats, stageReviewedFiles,
} from '../src/lib/builtin.mjs';
import { loadLearning } from '../src/lib/learn.mjs';
import { readStatus, formatStatus } from '../src/lib/status.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';
import { buildPublishMessage } from '../src/lib/publication.mjs';
import { parseRecipe } from '../src/lib/recipe.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { renderIssueBody } from '../src/lib/issue.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { packAgentRun } from '../vendor/github-agent-contracts/scripts/parse-agent-run.mjs';
import { withResearchSummary } from './helpers/research.mjs';
import { recordedCoderRun } from '../src/lib/seat-publication.mjs';
import { createDebugLog } from '../src/lib/debug-log.mjs';
import { readLocalRun } from '../src/lib/local-runs.mjs';

function runBuiltinIssue(issue, options) {
  return runIssueWithSeats(issue, { ...options, fetchImpl: withResearchSummary(options.fetchImpl) });
}

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const stubConfig = parseConfig(example);
const llmConfig = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
  .replace('model: ""', 'model: local-model'));
const vllmConfig = parseConfig(example.replace('profile: ""', 'profile: vllm-local')
  .replace('model: ""', 'model: local-model'));

function git(cwd, ...args) {
  return execFileSync('git', args, { cwd, encoding: 'utf8' }).trim();
}

function fixture(context) {
  const base = mkdtempSync(path.join(tmpdir(), 'roster-builtin-'));
  context.after(() => rmSync(base, { recursive: true, force: true }));
  const repoRoot = path.join(base, 'roster');
  const target = path.join(base, 'project');
  const contracts = path.join(base, 'contracts');
  mkdirSync(repoRoot);
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'),
    readFileSync(new URL('../principals/coder.md', import.meta.url), 'utf8'));
  writeFileSync(path.join(repoRoot, 'principals', 'reviewer.md'),
    readFileSync(new URL('../principals/reviewer.md', import.meta.url), 'utf8'));
  mkdirSync(target);
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  cpSync(new URL('../examples/', import.meta.url), path.join(repoRoot, 'examples'), { recursive: true });
  mkdirSync(path.join(contracts, 'scripts'), { recursive: true });
  writeFileSync(path.join(repoRoot, 'roster.config.example.yml'), example);
  writeFileSync(path.join(repoRoot, 'skills', 'implement-task', 'SKILL.md'), '# Code and test\n');
  writeFileSync(path.join(contracts, 'scripts', 'agent-pr.mjs'), 'export {};\n');
  writeFileSync(path.join(target, '.gitignore'), '.env\n.worktrees/\n.roster/runs/\n.roster/logs/\n.roster/fleet.yml\n.roster/config.yml\n');
  writeFileSync(path.join(target, 'AGENTS.md'), '# Agent instructions\nStay in the worktree.\n');
  writeFileSync(path.join(target, 'README.md'), '# Example\n');
  writeFileSync(path.join(target, 'smoke.test.mjs'),
    "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    "test('smoke', () => assert.equal(1, 1));\n");
  git(target, 'init', '-b', 'main');
  git(target, 'add', '--all');
  git(target, '-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '-m', 'Fixture setup');
  git(target, 'remote', 'add', 'origin', 'https://github.com/example/project.git');
  const cwd = path.join(target, 'nested');
  mkdirSync(cwd);
  const env = { ...process.env, ROSTER_MODEL: '', AI_MODEL: '', AI_MODEL_VERSION: '',
    GITHUB_APP_ID: undefined, GITHUB_APP_PRIVATE_KEY_PATH: undefined,
    GITHUB_AGENT_CONTRACTS: contracts };
  const issue = {
    number: 42, title: 'Add Status to README',
    body: 'Add a Status section to README.md.\n\n## Acceptance checks\n' +
      '- node --test exits 0\n- README has a Status section\n\n## Files allowed\n- `README.md`\n',
    url: 'https://github.com/example/project/issues/42',
  };
  const calls = [];
  let stderr = '';
  const errorOutput = { write(text) { stderr += String(text); } };
  const runCommand = async (program, args, workingDirectory) => {
    calls.push({ program, args, workingDirectory });
    if (program === 'gh') return JSON.stringify(issue);
    return git(workingDirectory, ...args);
  };
  return { base, repoRoot, target, cwd, env, contracts, issue, calls, runCommand, errorOutput,
    get stderr() { return stderr; } };
}

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
  assert.match(options.stderr, /^Writing the plan: outcome, allowed files, and checks\.$/m);
  assert.match(options.stderr, /^Preparing the task summary\.$/m);
  assert.match(options.stderr, /^Checking the diff against the task\.$/m);
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
  assert.deepEqual(options.calls.map(({ program }) => program), ['git', 'git', 'gh', 'git', 'git', 'git', 'git']);
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
  const result = await runBuiltinIssue(42, {
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
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
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
  assert.doesNotMatch(options.stderr, /Writing the plan/);
  assert.match(options.stderr, /Drafting the change/);
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
  assert.match(logs.join('\n'), /Task summary:\nOutcome: .+\nAllowed files: README\.md\nChecks:\n- .+\n- .+\nEffort: [lmhx]/);
  assert.doesNotMatch(logs.join('\n'), /then \/run 42/);
  const next = await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  assert.equal(next.planner.reused, true);
  assert.equal(next.planningOnly, undefined);
  assert.equal(next.result.mode, 'stub');
});

test('test budget exhaustion runs all four repairs, then writes failing review without reviewer inference', async (context) => {
  const options = fixture(context);
  let tests = 0;
  let coderTurns = 0;
  let failed;
  await assert.rejects(runBuiltinIssue(42, { ...options, config: llmConfig, log: () => {},
    fetchImpl: async (_url, request) => {
      const system = JSON.parse(request.body).messages[0].content;
      if (system.startsWith('You are the builtin planner seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({ title: 'Add Status',
            acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'] }),
        } }] });
      }
      assert.ok(!system.startsWith('You are the builtin reviewer seat.'), 'Exhausted tests cannot request reviewer inference');
      coderTurns += 1;
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: 'Done.',
      } }] });
    },
    runTestCommand: async () => {
      tests += 1;
      assert.doesNotMatch(options.stderr, /Checking the diff/);
      throw Object.assign(new Error('tests failed'), { code: 1, stdout: 'not ok', stderr: 'assertion failed' });
    },
  }), (error) => {
    failed = error.result;
    return /Test repair budget \(4\) exhausted/.test(error.message);
  });
  assert.equal(tests, 5);
  assert.equal(coderTurns, 5);
  assert.equal(failed.repairBudgetExhausted, true);
  assert.equal(failed.review.verdict, 'fail');
  assert.match(readFileSync(failed.review.reviewPath, 'utf8'), /Verdict: fail[\s\S]*repair budget \(4\) exhausted/);
  for (let attempt = 1; attempt <= 4; attempt += 1) {
    assert.match(options.stderr, new RegExp(`Tests failed\\. Repair ${attempt} of 4\\.`));
  }
});

test('a repaired failing test passes excellence, read-only review, and publication staging without widening TASK', async (context) => {
    const options = fixture(context);
    let tests = 0;
    let coderTurns = 0;
    const result = await runBuiltinIssue(42, { ...options, config: llmConfig, log: () => {},
      fetchImpl: async (_url, request) => {
        const body = JSON.parse(request.body);
        const system = body.messages[0].content;
        if (system.startsWith('You are the builtin planner seat.')) {
          return Response.json({ choices: [{ finish_reason: 'stop', message: {
            role: 'assistant', content: JSON.stringify({ title: 'Add Status',
              acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'] }),
          } }] });
        }
        if (system.startsWith('You are the builtin reviewer seat.')) {
          assert.equal(tests, 2);
          assert.match(body.messages[1].content, /smoke\.test\.mjs/);
          return Response.json({ choices: [{ finish_reason: 'stop', message: {
            role: 'assistant', content: JSON.stringify({ verdict: 'pass',
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
    assert.doesNotMatch(result.planner.task, /Files allowed\n[\s\S]*- `smoke\.test\.mjs`/);
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
            title: 'Add Status to README', acceptance_checks: ['node --test exits 0'],
            files_allowed: ['README.md'],
          }),
        } }] });
      }
      assert.match(logs.join('\n'), /Task summary:[\s\S]*Allowed files: README\.md[\s\S]*Effort:/);
      if (system.startsWith('You are the builtin reviewer seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({ verdict: 'pass',
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
  assert.ok(tests > 0);
  assert.match(readFileSync(path.join(result.worktreePath, 'README.md'), 'utf8'), /## Status\nReady/);
  assert.equal(readFileSync(path.join(options.target, 'README.md'), 'utf8'), '# Example\n');
  assert.match(logs.join('\n'), /Human AI-Eval|human AI-Eval/);
});

test('docs slice retry after failed review raises one supported tier and never exceeds high', async (context) => {
  const options = fixture(context);
  options.issue.body = renderIssueBody(options.issue.body, { task_class: 'docs', difficulty: 1 });
  const config = { ...llmConfig, llm: { ...llmConfig.llm, model: 'deepseek-v4.1-flash',
    base_url: 'http://192.168.1.48:8888/v1' } };
  for (const effort of ['low', 'high', 'high', 'high', 'none']) {
    let coderTurns = 0;
    const activeConfig = effort === 'none' ? { ...config, llm: { ...config.llm, effort_override: 'none' } } : config;
    const result = await runIssueWithSeats(42, { ...options, config: activeConfig, log: () => {},
      fetchImpl: async (_url, request) => {
        const body = JSON.parse(request.body);
        const system = body.messages[0].content;
        if (system.startsWith('You are the builtin planner seat.')) {
          assert.equal(body.reasoning_effort, 'low');
          return Response.json({ choices: [{ message: { role: 'assistant', content: JSON.stringify({
            title: options.issue.title, acceptance_checks: ['node --test exits 0'],
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
        assert.equal(body.max_tokens, 2048);
        assert.doesNotMatch(system, /## Principal|## Seat memory|implement-task/);
        return Response.json({ choices: [{ finish_reason: coderTurns === 1 ? 'tool_calls' : 'stop',
          message: coderTurns === 1 ? { role: 'assistant', reasoning_content: 'PRIVATE_CODER_THINKING',
            tool_calls: [{ id: 'edit', type: 'function', function: { name: 'write_file',
              arguments: JSON.stringify({ path: 'README.md', content: `# Example\n\n## Status\n${effort} ${Date.now()}.\n` }) } }],
          } : { role: 'assistant', content: 'Changed the scoped README.', reasoning_content: 'PRIVATE_CODER_THINKING' },
        }] });
      }, runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
    });
    assert.equal(result.review.verdict, 'fail');
    assert.equal(result.runs.coder.metrics.effort, { low: 'l', high: 'h', max: 'x', none: '-' }[effort]);
    assert.equal(recordedCoderRun({ repoRoot: options.target, run: result.runs.coder }).line, result.runs.coder.line);
    assert.match(options.stderr, new RegExp(`Drafting at ${effort} effort\\. Model prior: strong\\.`));
    assert.doesNotMatch(readFileSync(path.join(options.repoRoot, '.roster', 'memory', 'coder.jsonl'), 'utf8'),
      /PRIVATE_CODER_THINKING|reasoning_content/);
  }
});

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
    assert.equal(error.result.review.verdict, 'fail');
    assert.match(error.result.review.content, /HTTP timeout[\s\S]*review was not completed/);
    return /Cold-start:[\s\S]*host may still be warming[\s\S]*not a bad TASK[\s\S]*Retry: roster run --issue 42/.test(error.message);
  });
  assert.equal(readFileSync(initial.taskPath, 'utf8'), task);
  assert.equal(readFileSync(initial.recipePath, 'utf8'), recipe);
  assert.doesNotMatch(task, /Planning failure/);
  assert.match(options.stderr, /The model did not answer in time\. It may still be waking\./);
  assert.match(readFileSync(initial.logPath, 'utf8'), /host may still be warming[\s\S]*Retry: roster run --issue 42/);
  const logs = [];
  const retried = await runBuiltinIssue(42, { ...options, config: llmConfig, log: (text) => logs.push(text),
    fetchImpl: async (_url, request) => {
      assert.doesNotMatch(JSON.parse(request.body).messages[0].content, /builtin planner seat/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'No change needed.' } }] });
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
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
  assert.match(options.stderr, /Writing the plan:[\s\S]*Saving PLAN\.md\./);
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

test('feature planner writes five child issue drafts with wave labels and --publish cannot start coder', async (context) => {
  const options = fixture(context);
  options.issue.title = 'Implement a profile feature';
  options.issue.body = 'Implement a profile feature.\n\n## Allowed files\n- `README.md`';
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  git(options.target, 'worktree', 'add', '-b', 'issue-42', worktree);
  const before = readFileSync(path.join(worktree, 'README.md'));
  let calls = 0;
  const result = await runBuiltinIssue(42, { ...options, config: llmConfig, publish: true, log: () => {},
    env: { ...options.env, GITHUB_APP_ID: 'test-app', GITHUB_APP_PRIVATE_KEY_PATH: 'test-only.pem' },
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
    publisher: () => assert.fail('Feature cannot publish even with --publish'),
    issueCommenter: () => assert.fail('Feature children remain drafts'),
  });
  assert.equal(calls, 1);
  assert.equal(result.askKind, 'feature');
  assert.equal(result.planningOnly, true);
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

test('garbage planner arguments retry once then write visible stubs without coding or publishing', async (context) => {
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
  assert.equal(result.result.mode, 'stub');
  assert.equal(result.command, null);
  assert.match(readFileSync(result.taskPath, 'utf8'), /## Planning failure[\s\S]*after one retry/);
  assert.deepEqual(parseRecipe(readFileSync(result.recipePath, 'utf8')).seats.map(({ id }) => id),
    ['planner', 'coder', 'reviewer']);
  assert.equal(readFileSync(path.join(result.worktreePath, 'README.md'), 'utf8'), originalReadme);
  assert.match(logs.join('\n'), /Planning failed:[\s\S]*Publication skipped/);
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
            title: options.issue.title, acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'],
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
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
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

test('--publish requires an LLM and App environment before any GitHub or worktree action', async (context) => {
  const options = fixture(context);
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: stubConfig, publish: true, skipReview: true,
    env: { ...options.env, AI_MODEL: 'reviewed-model',
      GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
  }), /requires an LLM endpoint/);
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, publish: true,
    env: { ...options.env, GITHUB_APP_ID: undefined, GITHUB_APP_PRIVATE_KEY_PATH: undefined },
  }), /GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH/);
  assert.deepEqual(options.calls, []);
});

test('publish.enabled false blocks publication before GitHub, worktree creation, or the SDK', async (context) => {
  const options = fixture(context);
  const config = { ...llmConfig, publish: { enabled: false } };
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config, publish: true, skipReview: true,
    env: { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
    publisher: () => assert.fail('Disabled publication must not invoke the SDK'),
  }), /Publishing is disabled by publish\.enabled/);
  assert.deepEqual(options.calls, []);
  assert.equal(existsSync(path.join(options.target, '.worktrees')), false);
  await assert.rejects(prepareBuiltinPublication({}, { config }), /Publishing is disabled/);
});

test('--publish refuses an absent model before invoking GitHub, the SDK, or worktree preparation', async (context) => {
  const options = fixture(context);
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: stubConfig, publish: true,
    env: { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
    publisher: () => assert.fail('Missing-model publication must not invoke the SDK'),
  }), /set model/);
  assert.deepEqual(options.calls, []);
  assert.equal(existsSync(path.join(options.target, '.worktrees')), false);
});

test('--auto-model needs a registered fleet rather than nominating a model outside the catalog', async (context) => {
  const options = fixture(context);
  const emptyModel = parseConfig(example.replace('profile: ""', 'profile: ollama'));
  await assert.rejects(runBuiltinIssue(42, { ...options, config: emptyModel }),
    /set model/);
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, autoModel: true,
  }), /--auto-model requires at least one registered fleet profile/);
  assert.deepEqual(options.calls, []);
});

test('auto-model without qualifying evaluations, priors or matching hints stays a network-free stub', async (context) => {
  const options = fixture(context);
  options.issue.title = 'feat: Add status';
  const config = parseConfig(example.replace('profile: ""', 'profile: ollama'));
  mkdirSync(path.join(options.target, '.roster'));
  writeFileSync(path.join(options.target, '.roster', 'fleet.yml'), formatFleet({ profiles: [{
    id: 'candidate', base_url: 'https://candidate.example.invalid/v1', model: 'candidate-model',
    provider: 'vllm', context_max: 32768, concurrency: 1, hardware: 'test-gpu', notes: '',
  }] }));
  const evaluated = Array.from({ length: 2 }, (_, index) => ({
    task_class: 'feat', model: 'candidate-model', effort: 'h',
    evaluation: { session: `sample-${index}`, verdict: 'accept', difficulty: 3 },
  }));
  const logs = [];
  const result = await runBuiltinIssue(42, {
    ...options, config, autoModel: true, log: (line) => logs.push(line),
    metricsLoader: ({ contractsPath, cwd }) => {
      assert.equal(contractsPath, options.contracts);
      assert.equal(cwd, options.target);
      return evaluated;
    },
    fetchImpl: () => assert.fail('Insufficient data must not contact an LLM'),
    runTestCommand: () => assert.fail('Stub must not run tests'),
  });
  assert.equal(result.autoRecommendation, null);
  assert.equal(result.result.mode, 'stub');
  assert.equal(result.runs.coder, null);
  assert.equal(config.llm.model, '');
  assert.ok(logs.some((line) => line.includes('no eligible fleet profile or evidence')));
});

test('ROSTER_MODEL selects the same served model for both seats and their metadata', async (context) => {
  const options = fixture(context);
  const config = parseConfig(example.replace('profile: ""', 'profile: ollama'));
  let requests = 0;
  const result = await runBuiltinIssue(42, {
    ...options, config, log: () => {},
    env: { ...options.env, ROSTER_MODEL: 'served-model', ROSTER_API_KEY: 'test-only-key' },
    fetchImpl: async (_url, request) => {
      requests += 1;
      assert.equal(JSON.parse(request.body).model, 'served-model');
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: requests === 1 ? JSON.stringify({
            title: 'Add status', acceptance_checks: ['node --test exits 0'],
            files_allowed: ['README.md'],
          }) : 'Done.',
        } }],
      }) };
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.equal(requests, 2);
  assert.equal(result.runs.planner.env.AI_MODEL, 'served-model');
  assert.equal(result.runs.coder.env.AI_MODEL, 'served-model');
  assert.deepEqual(loadLearning({ cwd: options.target }).runs.map(({ provider }) => provider),
    ['local', 'local', undefined]);
  assert.equal(result.review.verdict, 'fail');
  assert.equal(config.llm.model, '');
});

test('a failed planner journals its last measured response rather than aggregate or inherited metadata', async (context) => {
  const options = fixture(context);
  let requests = 0;
  const config = { ...llmConfig, planner: { ...llmConfig.planner, turn_budget: 2 } };
  const result = await runBuiltinIssue(42, {
    ...options, config, log: () => {},
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key', AI_MODEL: 'GPT-6.1-Sol',
      AI_PROVIDER: 'github-copilot', AI_MODEL_VERSION: 'stale|invalid',
      AI_CONTEXT_USED: '1000000', AI_CONTEXT_MAX: '1000000' },
    fetchImpl: async () => {
      requests += 1;
      return Response.json({
        model: `actual-planner-${requests}`,
        choices: [{ message: { role: 'assistant', content: '{' } }],
        usage: requests === 1 ? { prompt_tokens: 3, completion_tokens: 2 }
          : { prompt_tokens: 100, completion_tokens: 40 },
      });
    },
  });
  assert.equal(result.failed, true);
  assert.equal(result.result.mode, 'stub');
  assert.equal(requests, 2);
  const records = loadLearning({ cwd: options.target }).runs;
  assert.deepEqual(records[0], {
    session: 'roster-42-planner', task: 'issue-42', provider: 'local', task_class: 'feat',
    model: 'actual-planner-2', effort: 'm', prompt_tokens: 100, completion_tokens: 40,
    context_used: 100, context_out: 40,
  });
  assert.equal(records.slice(1).every(({ model }) => model === undefined), true);
});

test('live unprofiled seats report their backend and usage without inheriting Copilot provenance', async (context) => {
  const options = fixture(context);
  let requests = 0;
  const result = await runBuiltinIssue(42, {
    ...options, config: llmConfig, log: () => {},
    env: { ...options.env, AI_PROVIDER: 'github-copilot', ROSTER_API_KEY: 'test-only-key' },
    fetchImpl: async () => {
      requests += 1;
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: requests === 1 ? JSON.stringify({
            title: 'Add status', acceptance_checks: ['node --test exits 0'],
            files_allowed: ['README.md'],
          }) : 'Reviewed README.',
        } }],
        usage: { prompt_tokens: requests, completion_tokens: 2 },
      }) };
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.equal(requests, 2);
  assert.equal(result.runs.coder.provider, 'local');
  assert.match(result.runs.coder.line, /^1\|local\|local-model@-\|/);
  const records = loadLearning({ cwd: options.target }).runs;
  assert.deepEqual(records.slice(0, 2).map(({ provider, model, context_used, context_out }) =>
    ({ provider, model, context_used, context_out })), [
    { provider: 'local', model: 'local-model', context_used: 1, context_out: 2 },
    { provider: 'local', model: 'local-model', context_used: 2, context_out: 2 },
  ]);
  assert.equal(records[2].model, undefined);
  assert.equal(existsSync(path.join(options.target, '.roster', 'evals.jsonl')), false);
});

for (const selection of ['explicit', 'feedback']) {
  test(`task metadata selects the coder model and estimate before coding (${selection})`, async (context) => {
    const options = fixture(context);
    mkdirSync(path.join(options.target, '.roster'), { recursive: true });
    writeFileSync(path.join(options.target, '.roster', 'evals.jsonl'), [60, 25, 10].map((minutes, index) =>
      JSON.stringify({ session: `previous-${index}`, model: 'task-model', task_class: 'fix', effort: 'h',
        verdict: 'accept', difficulty: 4, again: true, minutes })).join('\n') + '\n');
    let requests = 0;
    const result = await runBuiltinIssue(42, {
      ...options, config: llmConfig, log: () => {},
      env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
      fetchImpl: async (_url, request) => {
        requests += 1;
        const body = JSON.parse(request.body);
        assert.equal(body.model, requests === 1 ? 'local-model' : 'task-model');
        if (requests === 2) {
          assert.match(body.messages[0].content, /# Outcome:[\s\S]*## Checks/);
          assert.doesNotMatch(body.messages[0].content, /## Prior feedback|## Principal/);
          assert.match(readFileSync(path.join(options.target, '.worktrees', 'issue-42', 'ESTIMATE.md'), 'utf8'),
            new RegExp(`Source: ${selection === 'explicit' ? 'history' : 'recommendation'}`));
        }
        return { status: 200, json: async () => ({
          choices: [{ finish_reason: 'stop', message: { role: 'assistant',
            content: requests === 1 ? JSON.stringify({
              title: 'Fix status', acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'],
              difficulty: 4, estimate_min: 90, task_class: 'fix', model: selection === 'explicit' ? 'task-model' : '',
            }) : 'Done.',
          } }],
        }) };
      },
      runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
    });
    assert.equal(requests, 2);
    assert.equal(result.runs.planner.env.AI_MODEL, 'local-model');
    assert.equal(result.runs.planner.env.AI_EFFORT, 'm');
    assert.equal(result.runs.coder.env.AI_MODEL, 'task-model');
    assert.equal(result.runs.coder.env.AI_EFFORT, 'h');
  });
}

test('auto-model uses a three-evaluation recommendation for both seats without editing config', async (context) => {
  const options = fixture(context);
  options.issue.title = 'feat: Add status';
  const config = parseConfig(example.replace('profile: ""', 'profile: ollama'));
  mkdirSync(path.join(options.target, '.roster'));
  writeFileSync(path.join(options.target, '.roster', 'fleet.yml'), formatFleet({ profiles: [{
    id: 'candidate', base_url: 'https://candidate.example.invalid/v1', model: 'candidate-model',
    provider: 'vllm', context_max: 32768, concurrency: 1, hardware: 'test-gpu', notes: '',
  }] }));
  const evaluated = Array.from({ length: 3 }, (_, index) => ({
    task_class: 'feat', model: 'candidate-model', effort: 'h',
    evaluation: { session: `sample-${index}`, verdict: 'accept', difficulty: 3 },
  }));
  let requests = 0;
  const result = await runBuiltinIssue(42, {
    ...options, config, autoModel: true, metricsLoader: () => evaluated,
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    log: () => {},
    fetchImpl: async (url, request) => {
      requests += 1;
      assert.equal(String(url), 'https://candidate.example.invalid/v1/chat/completions');
      assert.equal(JSON.parse(request.body).model, 'candidate-model');
      if (requests === 1) return { status: 200, json: async () => ({
        choices: [{ message: { role: 'assistant', content: JSON.stringify({
          title: 'Add status', acceptance_checks: ['node --test exits 0'],
          files_allowed: ['README.md'],
        }) } }],
        usage: { prompt_tokens: 2, completion_tokens: 1 },
      }) };
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }],
        usage: { prompt_tokens: 3, completion_tokens: 2 },
      }) };
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.equal(requests, 2);
  assert.equal(result.autoRecommendation.n, 3);
  assert.equal(result.autoRecommendation.model, 'candidate-model');
  assert.equal(result.route.profile.id, 'candidate');
  assert.equal(result.route.source, 'evals');
  assert.equal(result.result.mode, 'llm');
  assert.equal(result.runs.planner.env.AI_MODEL, 'candidate-model');
  assert.equal(result.runs.coder.env.AI_MODEL, 'candidate-model');
  assert.equal(result.runs.coder.env.AI_EFFORT, 'm');
  assert.equal(result.runs.coder.env.AI_CONTEXT_USED, '3');
  assert.equal(config.llm.model, '');
  assert.equal(existsSync(path.join(options.repoRoot, '.roster', 'config.yml')), false);
});

test('fleet priors change endpoint/model only for explicit auto-model and never rewrite the saved default', async (context) => {
  for (const autoModel of [false, true]) {
    const options = fixture(context);
    options.issue.title = 'feat: Add status';
    mkdirSync(path.join(options.target, '.roster'));
    const configSource = example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
      .replace('model: ""', 'model: local-model');
    const configPath = path.join(options.target, '.roster', 'config.yml');
    writeFileSync(configPath, configSource);
    writeFileSync(path.join(options.target, '.roster', 'fleet.yml'), formatFleet({ profiles: [
      { id: 'default', base_url: llmConfig.llm.base_url, model: 'local-model',
        provider: 'vllm', context_max: 0, concurrency: 1, hardware: 'test-gpu', notes: '' },
      { id: 'burst', base_url: 'https://burst.example.invalid/v1', model: 'routed-model',
        provider: 'vllm', context_max: 32768, concurrency: 4,
        hardware: 'test-gpu', task_class: ['feat'], notes: '' },
    ] }));
    const logs = [];
    let requests = 0;
    const result = await runBuiltinIssue(42, {
      ...options, config: llmConfig, autoModel, metricsLoader: () => [],
      env: { ...options.env, ROSTER_API_KEY: 'test-only-key', OPENAI_API_KEY: 'unused-test-key' },
      log: (text) => logs.push(text),
      fetchImpl: async (url, request) => {
        requests += 1;
        const body = JSON.parse(request.body);
        assert.equal(url, autoModel ? 'https://burst.example.invalid/v1/chat/completions'
          : 'http://localhost:1234/v1/chat/completions');
        assert.equal(body.model, autoModel ? 'routed-model' : 'local-model');
        if (requests === 1) return { status: 200, json: async () => ({
          choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
            title: 'Add status', acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'],
          }) } }],
        }) };
        if (requests === 2) return { status: 200, json: async () => ({
          choices: [{ finish_reason: 'tool_calls', message: {
            role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
              name: 'write_file', arguments: JSON.stringify({
                path: 'README.md', content: '# Example\n\n## Status\nReady.\n',
              }),
            } }],
          } }],
        }) };
        return { status: 200, json: async () => ({
          choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Added status; tests pass.' } }],
        }) };
      },
      runTestCommand: async (_program, _args, { env }) => {
        assert.equal(env.ROSTER_API_KEY, undefined);
        assert.equal(env.OPENAI_API_KEY, undefined);
        return { stdout: 'pass', stderr: '' };
      },
    });
    assert.equal(requests, 3);
    assert.equal(readFileSync(configPath, 'utf8'), configSource);
    assert.equal(llmConfig.llm.model, 'local-model');
    assert.equal(result.runs.coder.env.AI_MODEL, autoModel ? 'routed-model' : 'local-model');
    if (autoModel) {
      assert.equal(result.route.source, 'prior');
      assert.equal(result.route.profile.id, 'burst');
      assert.match(logs[0], /profile=burst source=prior/);
      for (const seat of ['planner', 'coder', 'reviewer']) {
        assert.equal(result.runs[seat].metrics.context_max, 32768);
        assert.equal(result.runs[seat].env.AI_CONTEXT_MAX, '32768');
      }
    } else assert.equal(result.route, null);
  }
});

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
          acceptance_checks: ['node --test exits 0', 'README has a Status section'],
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
      assert.deepEqual(body.tools.map(({ function: tool }) => tool.name),
        ['read_file', 'write_file', 'list_dir', 'run_test', 'search_text']);
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
    publisher: async (program, args, publication) => {
      published += 1;
      assert.equal(program, process.execPath);
      assert.deepEqual(args, [
        path.join(options.contracts, 'scripts', 'agent-pr.mjs'),
        '--message', buildPublishMessage({
          subject: 'feat: issue 42', model: 'actual-coder-model',
          summary: 'Updated README; tests pass.', issueNumber: 42,
          seats: 'planner, coder, reviewer (pass)',
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
  assert.equal(result.result.tests.exit_code, 0);
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
    const options = fixture(context);
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
            files_allowed: ['README.md'],
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
      publish: true, skipReview, fetchImpl, log: () => {},
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
            title: 'Add status', acceptance_checks: ['node --test exits 0'],
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
          title: 'Add status', acceptance_checks: ['node --test exits 0'],
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
    runTestCommand: async () => ({ stdout: 'passed', stderr: '' }),
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

test('default planner/coder run preserves the task handoff while the coder edits only allowed code', async (context) => {
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
          write('code', 'src/app.mjs', 'export const ready = true;\n'),
          write('recipe', 'RECIPE.yml', 'tampered'),
          write('task', 'TASK.md', 'tampered'),
        ] },
      }] }) };
    }
    assert.equal(completion, 3);
    assert.match(body.messages.at(-3).content, /src\/app\.mjs/);
    assert.match(body.messages.at(-2).content, /not allowed/);
    assert.match(body.messages.at(-1).content, /not allowed/);
    return { ok: true, status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Implemented the planned task.' } }],
    }) };
  };
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, fetchImpl, log: () => {},
    vault: { get: async () => undefined },
    runTestCommand: () => assert.fail('Denied writes must stop before tests'),
  }), /Writing RECIPE\.yml is not allowed/);
  assert.equal(completion, 2);
  assert.deepEqual(parseRecipe(readFileSync(path.join(worktreePath, 'RECIPE.yml'), 'utf8')).seats.map(({ id }) => id),
    ['planner', 'coder', 'reviewer']);
  assert.equal(readFileSync(path.join(worktreePath, 'src', 'app.mjs'), 'utf8'),
    'export const ready = true;\n');
  assert.deepEqual(['RECIPE.yml', 'TASK.md'].map((name) => readFileSync(path.join(worktreePath, name), 'utf8')), handoff);
  assert.match(readFileSync(path.join(worktreePath, 'REVIEW.md'), 'utf8'), /Verdict: fail/);
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
        assert.match(options.stderr, /^Writing the plan: outcome, allowed files, and checks\.$/m);
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
      assert.deepEqual(body.tools.map(({ function: tool }) => tool.name),
        ['read_file', 'write_file', 'list_dir', 'run_test', 'search_text']);
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
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.equal(plannerTurns, 1);
  assert.equal(coderTurns, 2);
  assert.equal(result.result.excellence.pass, true);
  assert.equal(result.review.verdict, 'pass');
  const liveLog = readFileSync(result.logPath, 'utf8');
  assert.notEqual(liveLog, options.stderr);
  assert.match(options.stderr, /Reading|Saving README\.md\./);
  assert.match(options.stderr, /^Saving README\.md\.$/m);
  assert.match(options.stderr, /^Running tests\.$/m);
  assert.doesNotMatch(options.stderr, /http chat|model=|host=|elapsed_ms=|\d{4}-\d\d-\d\dT/);
  assert.match(liveLog, /seat planner tool write_file path="TASK\.md"/);
  assert.match(liveLog, /seat coder tool write_file path="README\.md"/);
  assert.match(liveLog, /seat coder tool run_test/);
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
    const options = fixture(context);
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
            files_allowed: ['README.md'],
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
  const options = fixture(context);
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
          files_allowed: ['README.md'], task_class: 'feat', difficulty: 4,
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
  const options = fixture(context);
  let published = false;
  const fetchImpl = async (_url, request) => {
    const body = JSON.parse(request.body);
    if (body.messages[0].content.startsWith('You are the builtin planner seat.')) {
      return { ok: true, status: 200, json: async () => ({ choices: [{ message: { role: 'assistant', content: JSON.stringify({
        title: 'Update README', acceptance_checks: ['node --test exits 0'],
        files_allowed: ['README.md'],
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
  assert.match(readFileSync(path.join(options.target, '.worktrees', 'issue-42', 'REVIEW.md'), 'utf8'),
    /Verdict: fail/);
  const records = loadLearning({ cwd: options.target }).runs;
  assert.deepEqual(records.map(({ session, excellence }) => ({ session, excellence })), [
    { session: 'roster-42-planner', excellence: undefined },
    { session: 'roster-42-coder', excellence: 'fail' },
    { session: 'roster-42-reviewer', excellence: undefined },
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
  const options = fixture(context);
  let requests = 0;
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, log: () => {},
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    fetchImpl: async () => {
      requests += 1;
      return { status: 200, json: async () => ({ choices: [{
        finish_reason: 'stop',
        message: { role: 'assistant', content: requests === 1 ? JSON.stringify({
          title: 'Update README', acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'],
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
  const options = fixture(context);
  let requests = 0;
  const run = await runBuiltinIssue(42, {
    ...options, config: llmConfig, log: () => {},
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    fetchImpl: async () => {
      requests += 1;
      return { status: 200, json: async () => ({ choices: [{
        finish_reason: 'stop', message: { role: 'assistant', content: requests === 1 ? JSON.stringify({
          title: 'Update README', acceptance_checks: ['node --test exits 0'], files_allowed: ['README.md'],
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
