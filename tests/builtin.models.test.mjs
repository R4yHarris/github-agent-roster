// Builtin seat orchestration: Publish preconditions, model selection, fleets, auto-model, and route recovery.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { passingReview, reviewedChecks } from './helpers/review.mjs';
import { withResearchSummary } from './helpers/research.mjs';
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
import { LlmTimeoutError, isLlmTimeout } from '../src/llm/request.mjs';
import { packAgentRun } from '../vendor/github-agent-contracts/scripts/parse-agent-run.mjs';
import { recordedCoderRun } from '../src/lib/seat-publication.mjs';
import { createDebugLog } from '../src/lib/debug-log.mjs';
import { readLocalRun } from '../src/lib/local-runs.mjs';
import { createSteeringControl } from '../src/runtime/steering.mjs';
import {
  runBuiltinIssue, example, stubConfig, llmConfig, vllmConfig, multiFileScope, git, fixture, multiFileFixture,
} from './helpers/builtin.mjs';

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
  const options = multiFileFixture(context);
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
            files_allowed: multiFileScope,
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
  assert.equal(result.planningOnly, true);
  assert.equal(result.result, undefined);
  assert.equal(result.review, undefined);
  assert.equal(result.runs.coder, null);
  assert.equal(result.runs.reviewer, null);
  assert.equal(requests, 2);
  const records = loadLearning({ cwd: options.target }).runs;
  assert.deepEqual(records[0], {
    session: 'roster-42-planner', task: 'issue-42', provider: 'local', task_class: 'feat',
    model: 'actual-planner-2', effort: 'm', prompt_tokens: 100, completion_tokens: 40,
    context_used: 100, context_out: 40,
  });
  assert.equal(records.length, 1);
});

test('live unprofiled seats report their backend and usage without inheriting Copilot provenance', async (context) => {
  const options = multiFileFixture(context);
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
            files_allowed: multiFileScope,
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
    const options = multiFileFixture(context);
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
              title: 'Fix status', acceptance_checks: ['node --test exits 0'], files_allowed: multiFileScope,
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
  const options = multiFileFixture(context);
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
          files_allowed: multiFileScope,
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
    const options = multiFileFixture(context);
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
            title: 'Add status', acceptance_checks: ['node --test exits 0'], files_allowed: multiFileScope,
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
      for (const seat of ['planner', 'coder']) {
        assert.equal(result.runs[seat].metrics.context_max, 32768);
        assert.equal(result.runs[seat].env.AI_CONTEXT_MAX, '32768');
      }
      assert.equal(result.runs.reviewer.env.AI_MODEL, 'local-model', 'the reviewer routes away from the coder profile');
    } else assert.equal(result.route, null);
  }
});

test('a fleet route records the routed model even when the gateway labels responses with another model', async (context) => {
  const options = multiFileFixture(context);
  options.issue.title = 'feat: Add status';
  twoProfileFleet(options.target);
  const requests = [];
  const logs = [];
  const result = await runBuiltinIssue(42, {
    ...options, config: llmConfig, autoModel: true, metricsLoader: () => [],
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    log: (line) => logs.push(line),
    fetchImpl: async (url, request) => {
      requests.push({ url: String(url), model: JSON.parse(request.body).model });
      if (requests.length === 1) return planReply('substituted-model');
      if (requests.length === 2) return editReply('substituted-model');
      return textReply('substituted-model', requests.length === 3 ? 'Added status; tests pass.' : 'PASS');
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.ok(requests.every(({ url, model }) => url === 'https://first.example.invalid/v1/chat/completions' &&
    model === 'first-model'));
  assert.deepEqual(result.routeAttempts, []);
  assert.equal(result.route.profile.id, 'first');
  for (const seat of ['planner', 'coder']) assert.equal(result.runs[seat].env.AI_MODEL, 'first-model');
  assert.equal(result.runs.reviewer.env.AI_MODEL, 'alternate-model', 'the reviewer is not the coder (spec 4.8)');
  assert.ok(logs.some((line) => line === 'Reviewer route: profile=alternate model=alternate-model (independent of coder profile=first).'));
  assert.equal(logs.some((line) => /substituted-model|quarantine/i.test(line)), false);
  assert.equal(existsSync(path.join(options.target, '.roster', 'runs', 'route-quarantine.json')), false);
});

const twoProfileFleet = (target) => {
  mkdirSync(path.join(target, '.roster'));
  writeFileSync(path.join(target, '.roster', 'fleet.yml'), formatFleet({ profiles: [
    {
      id: 'first', base_url: 'https://first.example.invalid/v1', model: 'first-model',
      provider: 'vllm', context_max: 32768, concurrency: 8,
      hardware: 'test-gpu', task_class: ['feat'], notes: '',
    },
    {
      id: 'alternate', base_url: 'https://alternate.example.invalid/v1', model: 'alternate-model',
      provider: 'vllm', context_max: 65536, concurrency: 1,
      hardware: 'test-gpu', task_class: ['feat'], notes: '',
    },
  ] }));
};
const planReply = (model) => Response.json({ model, choices: [{ finish_reason: 'stop', message: {
  role: 'assistant', content: JSON.stringify({
    title: 'Add status', acceptance_checks: ['node --test exits 0'], files_allowed: multiFileScope,
  }) } }] });
const editReply = (model) => Response.json({ model, choices: [{ finish_reason: 'tool_calls', message: {
  role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
    name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
  } }] } }] });
const textReply = (model, content) => Response.json({ model,
  choices: [{ finish_reason: 'stop', message: { role: 'assistant', content } }] });

for (const failure of ['timeout']) {
  test(`auto-model continues the coder seat on an alternate profile after a route ${failure}`, async (context) => {
    const options = multiFileFixture(context);
    options.issue.title = 'feat: Add status';
    twoProfileFleet(options.target);
    const requests = [];
    const logs = [];
    const result = await runBuiltinIssue(42, {
      ...options, config: llmConfig, autoModel: true, metricsLoader: () => [],
      env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
      log: (line) => logs.push(line),
      fetchImpl: async (url, request) => {
        const body = JSON.parse(request.body);
        requests.push({ url: String(url), model: body.model, prompt: JSON.stringify(body.messages) });
        if (requests.length === 1) return planReply('first-model');
        if (requests.length === 2) {
          throw new LlmTimeoutError({ host: 'first.example.invalid', timeoutMs: 1000, local: false });
        }
        if (requests.length === 3) return editReply('alternate-model');
        return textReply('alternate-model', requests.length === 4 ? 'Added status; tests pass.' : 'PASS');
      },
      runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
    });
    assert.equal(requests[1].model, 'first-model');
    assert.equal(requests[2].url, 'https://alternate.example.invalid/v1/chat/completions');
    assert.equal(requests[2].model, 'alternate-model');
    assert.match(requests[2].prompt, /previous coder attempt on fleet profile first stopped because of an endpoint route failure/);
    assert.equal(result.routeAttempts.length, 1);
    assert.equal(result.routeAttempts[0].seat, 'coder');
    assert.equal(result.routeAttempts[0].profile, 'first');
    assert.equal(result.routeAttempts[0].reason, 'endpoint-timeout');
    assert.equal(result.runs.planner.env.AI_MODEL, 'first-model');
    assert.equal(result.runs.coder.env.AI_MODEL, 'alternate-model');
    assert.equal(result.runs.reviewer.env.AI_MODEL, 'alternate-model');
    assert.ok(result.result.excellence.pass);
    assert.ok(logs.some((line) => /Route recovery: seat=coder profile=first .*profile=alternate model=alternate-model/.test(line)));
  });
}

test('auto-model reviews on an independent profile and falls back to the coder model when it fails', async (context) => {
  const options = multiFileFixture(context);
  options.issue.title = 'feat: Add status';
  twoProfileFleet(options.target);
  const requests = [];
  const reviews = [];
  const logs = [];
  const seatFetch = withResearchSummary(async (url, request) => {
    requests.push({ url: String(url), model: JSON.parse(request.body).model });
    if (requests.length === 1) return planReply('first-model');
    if (requests.length === 2) return editReply('first-model');
    return textReply('first-model', 'Added status; tests pass.');
  });
  const result = await runIssueWithSeats(42, {
    ...options, config: llmConfig, autoModel: true, metricsLoader: () => [],
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    log: (line) => logs.push(line),
    fetchImpl: async (url, request) => {
      const body = JSON.parse(request.body);
      if (!body.messages[0].content.startsWith('You are the builtin reviewer seat.')) return seatFetch(url, request);
      reviews.push({ url: String(url), model: body.model });
      if (reviews.length === 1) throw new LlmTimeoutError({ host: 'alternate.example.invalid', timeoutMs: 1000, local: false });
      return textReply('first-model', passingReview(body));
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.deepEqual(reviews, [
    { url: 'https://alternate.example.invalid/v1/chat/completions', model: 'alternate-model' },
    { url: 'https://first.example.invalid/v1/chat/completions', model: 'first-model' },
  ]);
  assert.deepEqual(result.routeAttempts.map(({ seat, profile }) => ({ seat, profile })),
    [{ seat: 'reviewer', profile: 'alternate' }]);
  assert.equal(result.route.profile.id, 'first', 'a reviewer endpoint failure never moves the coder route');
  assert.equal(result.runs.coder.env.AI_MODEL, 'first-model');
  assert.equal(result.runs.reviewer.env.AI_MODEL, 'first-model');
  assert.equal(result.review.verdict, 'pass');
  assert.ok(logs.some((line) => /Route recovery: seat=reviewer profile=alternate endpoint failed; no other independent profile/.test(line)));
});

test('auto-model reviewer uses a profile other than the coder when one is eligible', async (context) => {
  const options = multiFileFixture(context);
  options.issue.title = 'feat: Add status';
  twoProfileFleet(options.target);
  const reviews = [];
  let seatCalls = 0;
  const seatFetch = withResearchSummary(async () => {
    seatCalls += 1;
    if (seatCalls === 1) return planReply('first-model');
    if (seatCalls === 2) return editReply('first-model');
    return textReply('first-model', 'Added status; tests pass.');
  });
  const result = await runIssueWithSeats(42, {
    ...options, config: llmConfig, autoModel: true, metricsLoader: () => [],
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    log: () => {},
    fetchImpl: async (url, request) => {
      const body = JSON.parse(request.body);
      if (!body.messages[0].content.startsWith('You are the builtin reviewer seat.')) return seatFetch(url, request);
      reviews.push({ url: String(url), model: body.model });
      return textReply('alternate-model', passingReview(body));
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.deepEqual(reviews, [{ url: 'https://alternate.example.invalid/v1/chat/completions', model: 'alternate-model' }]);
  assert.deepEqual(result.routeAttempts, []);
  assert.equal(result.runs.coder.env.AI_MODEL, 'first-model');
  assert.equal(result.runs.reviewer.env.AI_MODEL, 'alternate-model');
  assert.equal(result.review.verdict, 'pass');
});

test('auto-model coder fails explicitly when no alternate fleet profile remains', async (context) => {
  const options = multiFileFixture(context);
  options.issue.title = 'feat: Add status';
  mkdirSync(path.join(options.target, '.roster'));
  writeFileSync(path.join(options.target, '.roster', 'fleet.yml'), formatFleet({ profiles: [{
    id: 'burst', base_url: 'https://burst.example.invalid/v1', model: 'routed-model',
    provider: 'vllm', context_max: 32768, concurrency: 1,
    hardware: 'test-gpu', task_class: ['feat'], notes: '',
  }] }));
  const logs = [];
  let requests = 0;
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, autoModel: true, metricsLoader: () => [],
    env: { ...options.env, ROSTER_API_KEY: 'test-only-key' },
    log: (line) => logs.push(line),
    fetchImpl: async () => {
      if (++requests === 1) return planReply('routed-model');
      throw new LlmTimeoutError({ host: 'burst.example.invalid', timeoutMs: 1000, local: false });
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  }), (error) => isLlmTimeout(error));
  assert.ok(logs.some((line) => /Route recovery exhausted: seat=coder profile=burst/.test(line)));
});
