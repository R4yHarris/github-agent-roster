import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { writeAsk } from '../src/lib/ask.mjs';
import {
  prepareBuiltinPublication, runBuiltinIssue as runIssueWithSeats, stageReviewedFiles,
} from '../src/lib/builtin.mjs';
import { loadLearning } from '../src/lib/learn.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';
import { buildPublishMessage } from '../src/lib/publication.mjs';
import { parseRecipe } from '../src/lib/recipe.mjs';
import { renderIssueBody } from '../src/lib/issue.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { packAgentRun } from '../vendor/github-agent-contracts/scripts/parse-agent-run.mjs';
import { withResearchSummary } from './helpers/research.mjs';

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
  writeFileSync(path.join(target, '.gitignore'), '.env\n.worktrees/\n.roster/runs/\n.roster/fleet.yml\n.roster/config.yml\n');
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
  const runCommand = async (program, args, workingDirectory) => {
    calls.push({ program, args, workingDirectory });
    if (program === 'gh') return JSON.stringify(issue);
    return git(workingDirectory, ...args);
  };
  return { base, repoRoot, target, cwd, env, contracts, issue, calls, runCommand };
}

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
  await assert.rejects(writeAsk('Another ask', { repoRoot, config: stubConfig, id: 'draft-1' }), /EEXIST/);
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
  assert.match(logs[0], /Publication unavailable: set model/);
  assert.equal((logs[0].match(/AI-Run:/g) ?? []).length, 0);
  assert.deepEqual(result.runs, { planner: null, coder: null, reviewer: null });
  assert.deepEqual(options.calls.map(({ program }) => program), ['git', 'git', 'gh', 'git']);
  assert.equal(options.calls.filter(({ program, args }) =>
    program === 'git' && args[0] === 'worktree').length, 1);
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
  await assert.rejects(runBuiltinIssue(42, {
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
  }), /Planner turn budget \(2\) exhausted/);
  assert.equal(requests, 2);
  const records = loadLearning({ cwd: options.target }).runs;
  assert.deepEqual(records, [{
    session: 'roster-42-planner', task: 'issue-42', provider: 'local',
    model: 'actual-planner-2', effort: 'm', prompt_tokens: 100, completion_tokens: 40,
    context_used: 100, context_out: 40,
  }]);
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
          assert.match(body.messages[0].content,
            /difficulty: 4\nestimate_min: 25\ntask_class: fix\nmodel: task-model\n/);
          assert.match(body.messages[0].content, /## Prior feedback/);
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
    assert.equal(result.runs.coder.env.AI_EFFORT, selection === 'explicit' ? 'm' : 'h');
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
  assert.equal(result.runs.coder.env.AI_EFFORT, 'h');
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
      assert.equal(body.tools, undefined);
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
      assert.match(body.messages[0].content, /## TASK\.md\n\n---\nskills: [^\n]+\n---\n# Task: Add Status to README/);
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
  assert.equal((logs[0].match(/AI-Run:/g) ?? []).length, 3);
  assert.match(logs[0], /AI_CONTEXT_MAX=\n/);
  assert.ok(logs.some((line) => line.includes('Merged PR #7')));
  assert.ok(!logs.join('\n').includes('private-key'));
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
  const fetchImpl = async (_url, request) => {
    completion += 1;
    const body = JSON.parse(request.body);
    if (completion === 1) {
      assert.equal(body.tools, undefined);
      return { ok: true, status: 200, json: async () => ({ choices: [{ message: { role: 'assistant', content: JSON.stringify({
        title: 'Implement the app',
        acceptance_checks: ['node --test exits 0'],
        files_allowed: ['src/app.mjs'],
      }) } }] }) };
    }
    if (completion === 2) {
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
  const run = await runBuiltinIssue(42, {
    ...options, config: llmConfig, fetchImpl, log: () => {},
    vault: { get: async () => undefined },
    runTestCommand: async () => ({ stdout: 'tests passed', stderr: '' }),
  });
  assert.equal(completion, 3);
  assert.deepEqual(parseRecipe(readFileSync(run.recipePath, 'utf8')).seats.map(({ id }) => id),
    ['planner', 'coder', 'reviewer']);
  assert.equal(readFileSync(path.join(run.worktreePath, 'src', 'app.mjs'), 'utf8'),
    'export const ready = true;\n');
  assert.equal(readFileSync(run.recipePath, 'utf8'), run.planner.recipe);
  assert.equal(readFileSync(run.taskPath, 'utf8'), run.planner.task);
  assert.equal(options.calls.filter(({ program, args }) =>
    program === 'git' && args[0] === 'worktree').length, 1);
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
    assert.equal(vaultReads, source === 'environment' ? 0 : 3);
    assert.equal(result.runs.planner.env.AI_CONTEXT_USED, '3');
    assert.equal(result.runs.coder.env.AI_CONTEXT_USED, '5');
    assert.ok(!logs.join('\n').includes(key));
    assert.ok(!readFileSync(path.join(options.repoRoot, '.roster', 'memory', 'coder.jsonl'),
      'utf8').includes(key));
  }
});

test('planner and coder read only their own last 20 memory lines and append separately', async (context) => {
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
    const expected = calls === 1 ? 'planner' : 'coder';
    const other = calls === 1 ? 'coder' : 'planner';
    const context = body.messages[calls === 1 ? 1 : 0].content;
    assert.match(context, new RegExp(`"seat":"${expected}","index":5`));
    assert.match(context, new RegExp(`"seat":"${expected}","index":24`));
    assert.doesNotMatch(context, new RegExp(`"seat":"${expected}","index":4`));
    assert.doesNotMatch(context, new RegExp(`"seat":"${other}"`));
    if (calls === 1) {
      return { status: 200, json: async () => ({ choices: [{ message: {
        role: 'assistant', content: JSON.stringify({
          title: 'Add status', acceptance_checks: ['node --test exits 0'],
          files_allowed: ['README.md'],
        }),
      } }] }) };
    }
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
    if (!body.tools) {
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
