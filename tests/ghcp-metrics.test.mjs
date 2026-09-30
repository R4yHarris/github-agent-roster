import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import {
  buildPublishMessage, formatPublishEnvironment, parsePublishArgs, publicationTask,
} from '../src/lib/publication.mjs';
import { buildGhcpRun, buildPublishEnv, buildRun } from '../src/metrics/run.mjs';
import { packAgentRun, parseAgentRun } from '../vendor/github-agent-contracts/scripts/parse-agent-run.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const configured = parseConfig(example.replace('profile: ""', 'profile: vllm-local')
  .replace('model: ""', 'model: requested-seat-model'));
const ghcpEnv = {
  AI_MODEL: 'GPT-6.1-Sol', AI_PROVIDER: 'local', AI_MODEL_VERSION: 'stale', AI_EFFORT: 'max',
  AI_CONTEXT_USED: '1000000', AI_CONTEXT_OUT: '999', AI_CONTEXT_MAX: '1000000',
  AI_SESSION: 'roster-stale-coder', AI_TASK: 'feat-ghcp-metrics',
};

test('GHCP packs exactly unknown used/output, declared capacity, explicit model, and Max effort', () => {
  const run = buildGhcpRun({ env: ghcpEnv, session: 'ghcp-20260930' });
  assert.equal(run.line,
    '1|github-copilot|GPT-6.1-Sol@-|x|-/1000000|-|ghcp-20260930|feat-ghcp-metrics');
  assert.equal(packAgentRun(run.env), run.line);
  assert.deepEqual(run.metrics, {
    provider: 'github-copilot', model: 'GPT-6.1-Sol', effort: 'x', context_max: 1000000,
    session: 'ghcp-20260930', task: 'feat-ghcp-metrics',
  });
  for (const field of ['AI_CONTEXT_USED', 'AI_CONTEXT_OUT']) {
    assert.equal(Object.hasOwn(run.env, field), false);
  }
  assert.equal(Object.isFrozen(run.metrics), true);
});

test('GHCP without declared capacity or effort cannot borrow either from an LLM profile', () => {
  const config = { ...configured, llm: { ...configured.llm, effort: 'h', context_max: 8192 } };
  const env = buildPublishEnv({ config, env: { AI_MODEL: 'GPT-6.1-Sol',
    AI_CONTEXT_USED: '1000000', AI_CONTEXT_OUT: '1000000' }, task: 'feat-ghcp-metrics' });
  assert.equal(env.AI_PROVIDER, 'github-copilot');
  assert.equal(env.AI_EFFORT, '-');
  assert.equal(env.AI_CONTEXT_MAX, undefined);
  const parsed = parseAgentRun(packAgentRun(env));
  assert.equal(parsed.context_used, null);
  assert.equal(parsed.context_max, null);
  assert.equal(parsed.context_out, null);
  assert.match(parsed.session, /^ghcp-\d+$/);
});

test('GHCP explicit publication model wins over AI_MODEL but served-model fallbacks are forbidden', () => {
  const env = buildPublishEnv({ config: configured,
    env: { ...ghcpEnv, AI_MODEL: 'unknown' }, model: 'explicit-copilot-model' });
  assert.equal(env.AI_MODEL, 'explicit-copilot-model');
  assert.equal(env.AI_PROVIDER, 'github-copilot');
  for (const model of [undefined, '', 'unknown', 'bad|model']) {
    assert.throws(() => buildPublishEnv({
      config: configured, env: { AI_MODEL: model, ROSTER_MODEL: 'served-model' },
    }), /GHCP publication requires --model or AI_MODEL/);
  }
});

test('GHCP validates declarations without fabricating tokens or losing large declared capacities', () => {
  const huge = buildGhcpRun({ env: { AI_MODEL: 'real-model', AI_CONTEXT_MAX: '9007199254740993',
    AI_SESSION: 'ghcp-123', AI_TASK: '' } });
  assert.equal(huge.metrics.context_max, '9007199254740993');
  assert.equal(parseAgentRun(huge.line).context_max, '9007199254740993');
  assert.equal(huge.env.AI_TASK, undefined);
  for (const values of [
    { AI_EFFORT: 'invalid' }, { AI_CONTEXT_MAX: '0' }, { AI_CONTEXT_MAX: '-1' },
    { AI_CONTEXT_MAX: 1000000 }, { AI_SESSION: 'ghcp-invalid session' }, { AI_TASK: 'bad/task' },
  ]) {
    assert.throws(() => buildGhcpRun({ env: { AI_MODEL: 'real-model', ...values } }),
      /AI_EFFORT|AI_CONTEXT_MAX|AI_SESSION|AI_TASK/);
  }
});

test('completed vLLM and cloud objects override GHCP model flags and process declarations', () => {
  for (const provider of ['vllm', 'openai']) {
    const config = { ...configured, llm: { ...configured.llm, provider, context_max: 8192, effort: 'h' } };
    const run = buildRun({ config, response: {
      model: 'actual-response-model', usage: { prompt_tokens: 100, completion_tokens: 40 },
    }, session: 'roster-42-coder', task: 'issue-42', env: ghcpEnv });
    const env = buildPublishEnv({ config: configured, env: ghcpEnv, run,
      model: 'explicit-copilot-model', session: 'ghcp-123', task: 'different-task' });
    assert.equal(packAgentRun(env), run.line);
    assert.equal(env.AI_PROVIDER, provider === 'vllm' ? 'local' : 'openai');
    assert.equal(env.AI_MODEL, 'actual-response-model');
    assert.equal(env.AI_EFFORT, 'h');
    assert.equal(env.AI_CONTEXT_USED, '100');
    assert.equal(env.AI_CONTEXT_OUT, '40');
    assert.equal(env.AI_CONTEXT_MAX, '8192');
    assert.equal(env.AI_SESSION, 'roster-42-coder');
    assert.equal(env.AI_TASK, 'issue-42');
  }
});

test('a measured seat without usage remains a seat, never a GHCP success-shaped fallback', () => {
  const run = buildRun({ config: configured, response: { model: 'served-model', usage: null },
    session: 'roster-42-coder', task: 'issue-42', env: ghcpEnv });
  const env = buildPublishEnv({ config: configured, env: ghcpEnv, run, model: 'other-copilot-model' });
  assert.equal(env.AI_MODEL, 'served-model');
  assert.equal(env.AI_PROVIDER, 'local');
  assert.equal(env.AI_SESSION, 'roster-42-coder');
  for (const field of ['AI_CONTEXT_USED', 'AI_CONTEXT_OUT', 'AI_CONTEXT_MAX']) {
    assert.equal(Object.hasOwn(env, field), false);
  }
});

test('publish parser accepts an explicit model and review flag in either order and rejects malformed flags', () => {
  assert.deepEqual(parsePublishArgs('fix: metadata --model GPT-6.1-Sol --skip-review'),
    { subject: 'fix: metadata', model: 'GPT-6.1-Sol', skipReview: true });
  assert.deepEqual(parsePublishArgs('--skip-review --model GPT-6.1-Sol fix: metadata'),
    { subject: 'fix: metadata', model: 'GPT-6.1-Sol', skipReview: true });
  for (const args of ['--model', '--model unknown', '--model real --model other',
    '--skip-review --skip-review', '--unknown']) {
    assert.throws(() => parsePublishArgs(args), /Use \/publish|set model|only once/);
  }
  assert.throws(() => parsePublishArgs(null), /must be text/);
});

test('GHCP task comes from an issue ID or safe branch slug and detached HEAD needs an explicit task', () => {
  const unexpected = () => assert.fail('Known task must not read Git');
  assert.equal(publicationTask({ env: { AI_TASK: 'issue-42' }, run: unexpected }), 'issue-42');
  assert.equal(publicationTask({ env: {}, task: 'issue-42', run: unexpected }), 'issue-42');
  const slug = publicationTask({ env: {}, cwd: 'fixture-worktree', run(program, args, options) {
    assert.equal(program, 'git');
    assert.deepEqual(args, ['symbolic-ref', '--short', 'HEAD']);
    assert.equal(options.cwd, 'fixture-worktree');
    return 'feat/ghcp-metrics\n';
  } });
  assert.equal(slug, 'feat-ghcp-metrics');
  assert.throws(() => publicationTask({ env: {}, run: () => { throw new Error('detached'); } }),
    /checked-out branch or an explicit AI_TASK/);
  assert.throws(() => publicationTask({ env: { AI_TASK: 'bad/task' }, run: unexpected }),
    /opaque branch slug/);
});

test('GHCP body and manual environment explicitly communicate unknown used/output', () => {
  const env = buildPublishEnv({ config: configured, env: ghcpEnv });
  const assignments = formatPublishEnvironment(env);
  assert.match(assignments, /AI_PROVIDER=github-copilot\n/);
  assert.match(assignments, /AI_CONTEXT_USED=\n/);
  assert.match(assignments, /AI_CONTEXT_OUT=\n/);
  assert.match(assignments, /AI_CONTEXT_MAX=1000000\n/);
  const message = buildPublishMessage({
    subject: 'feat: GHCP metadata', model: env.AI_MODEL, summary: 'Record declared session metadata.', ghcp: true,
  });
  assert.match(message, /## Model\n\nGPT-6\.1-Sol/);
  assert.match(message, /GHCP used\/out are `-` \(unknown\)/);
  assert.doesNotMatch(buildPublishMessage({
    subject: 'feat: seat metadata', model: 'served-model', summary: 'Record measured metadata.',
  }), /GHCP used\/out/);
});
