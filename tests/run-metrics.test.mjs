import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { buildPublishEnv, buildRun, mergeUsage, RUN_ENV_NAMES } from '../src/metrics/run.mjs';
import { packAgentRun, parseAgentRun } from '../vendor/github-agent-contracts/scripts/parse-agent-run.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
  .replace('model: ""', 'model: owner/model').replace('context_max: 0', 'context_max: 8192'));

test('builds a contracts-compatible compact AI-Run line from known token counts', () => {
  const usage = mergeUsage({ prompt_tokens: 12, completion_tokens: 5 },
    { prompt_tokens: 33, completion_tokens: 11 });
  const run = buildRun({ config, usage, session: 'roster-session', task: 'issue-42', env: {} });
  assert.equal(run.line, '1|local|owner/model@-|m|45/8192|16|roster-session|issue-42');
  assert.equal(run.provider, 'local');
  assert.deepEqual(run.env, {
    AI_PROVIDER: 'local', AI_MODEL: 'owner/model', AI_MODEL_VERSION: '-',
    AI_EFFORT: 'm', AI_CONTEXT_MAX: '8192',
    AI_CONTEXT_USED: '45', AI_CONTEXT_OUT: '16',
    AI_SESSION: 'roster-session', AI_TASK: 'issue-42',
  });
  assert.equal(packAgentRun(run.env), run.line);
  assert.equal(parseAgentRun(run.line, config.llm.model).context_used, 45);
});

test('records backend provenance while packing only contracts-supported AI-Run providers', () => {
  const vllm = parseConfig(example.replace('profile: ""', 'profile: vllm-local')
    .replace('model: ""', 'model: served-model'));
  const vllmRun = buildRun({
    config: vllm, env: { AI_PROVIDER: 'github-copilot' },
    usage: { prompt_tokens: 13, completion_tokens: 4 }, session: 'coder-42', task: 'issue-42',
  });
  assert.equal(vllmRun.provider, 'vllm');
  assert.equal(vllmRun.env.AI_PROVIDER, 'local');
  assert.equal(parseAgentRun(vllmRun.line).provider, 'local');
  assert.equal(vllmRun.env.AI_CONTEXT_USED, '13');
  assert.equal(vllmRun.env.AI_CONTEXT_OUT, '4');

  const copilot = buildRun({ config, env: { AI_PROVIDER: 'github-copilot' } });
  assert.equal(copilot.provider, 'github-copilot');
  assert.equal(copilot.env.AI_PROVIDER, 'github-copilot');
  assert.equal(parseAgentRun(copilot.line).provider, 'github-copilot');
  assert.equal(buildRun({ config, env: { AI_PROVIDER: 'vllm' } }).env.AI_PROVIDER, 'local');
  const openai = parseConfig(example.replace('profile: ""', 'profile: openai')
    .replace('model: ""', 'model: remote-model'));
  assert.equal(buildRun({ config: openai, env: { AI_PROVIDER: 'github-copilot' } }).provider, 'openai');
  assert.throws(() => buildRun({ config, env: { AI_PROVIDER: 'invalid' } }), /AI_PROVIDER/);
  assert.equal(buildPublishEnv({
    config: parseConfig(example), env: { AI_MODEL: 'served-model', AI_PROVIDER: 'vllm' },
  }).AI_PROVIDER, 'local');
});

test('configured provider wins over inherited Copilot metadata while vLLM remains contracts-compatible', () => {
  const configured = { ...config, llm: { ...config.llm, provider: 'vllm' } };
  const run = buildRun({ config: configured, env: { AI_PROVIDER: 'github-copilot' } });
  assert.equal(run.provider, 'vllm');
  assert.equal(run.env.AI_PROVIDER, 'local');
  assert.equal(parseAgentRun(run.line).provider, 'local');
  const copilot = buildRun({
    config: { ...config, llm: { ...config.llm, provider: 'github-copilot' } }, env: {},
  });
  assert.equal(copilot.env.AI_PROVIDER, 'github-copilot');
  assert.throws(() => buildRun({
    config: { ...config, llm: { ...config.llm, provider: 'invalid' } }, env: {},
  }), /llm\.provider/);
  assert.throws(() => buildPublishEnv({
    config: { ...config, publish: { enabled: false } }, env: {},
  }), /Publishing is disabled by publish\.enabled/);
});

test('does not invent an unknown model, version, context, or token counts', () => {
  assert.equal(buildRun({ config: parseConfig(example), usage: {}, env: {} }), null);
  const partial = mergeUsage({ prompt_tokens: 7 }, { prompt_tokens: 5, completion_tokens: 2 });
  assert.deepEqual(partial, { prompt_tokens: 12 });
  const run = buildRun({ config: parseConfig(example.replace('base_url: ""', 'base_url: http://localhost/v1')
    .replace('model: ""', 'model: m')), usage: partial, env: {} });
  assert.equal(run.line, '1|local|m@-|m|12/-|-|-|-');
  assert.equal(run.env.AI_CONTEXT_OUT, undefined);
  assert.equal(run.env.AI_CONTEXT_MAX, undefined);
  assert.equal(run.env.AI_PROVIDER, 'local');
  assert.equal(packAgentRun(run.env), run.line);
  assert.deepEqual(mergeUsage(null, { prompt_tokens: 2 }), {});
});

test('publication refuses absent and unknown models instead of clearing metadata and publishing', () => {
  for (const env of [{}, Object.fromEntries(RUN_ENV_NAMES.map((name) => [name, 'unknown']))]) {
    assert.throws(() => buildPublishEnv({
      config: parseConfig(example), env, run: null,
    }), /set model/);
  }
});

test('publication uses config, AI_MODEL, then ROSTER_MODEL without forwarding stale usage or API keys', () => {
  for (const [configured, sessionModel] of [[true, 'GPT-6-Sol'], [false, 'GPT-6-Sol'], [false, '']]) {
    const env = buildPublishEnv({
      config: configured ? config : parseConfig(example),
      env: { ROSTER_MODEL: 'served-model', AI_MODEL: sessionModel, AI_PROVIDER: 'github-copilot',
        AI_MODEL_VERSION: 'v2', AI_EFFORT: 'x', AI_CONTEXT_OUT: '999',
        AI_SESSION: 'roster-42-coder', AI_TASK: 'issue-42', ROSTER_API_KEY: 'private-value' },
    });
    assert.equal(env.AI_MODEL, configured ? 'owner/model' : sessionModel || 'served-model');
    assert.equal(env.AI_MODEL_VERSION, 'v2');
    assert.equal(env.AI_PROVIDER, 'github-copilot');
    assert.equal(env.AI_EFFORT, 'm');
    assert.equal(env.AI_CONTEXT_OUT, undefined);
    assert.equal(env.ROSTER_API_KEY, undefined);
    assert.equal(parseAgentRun(packAgentRun(env)).session, 'roster-42-coder');
  }
});

test('publication keeps the completed coder model even if config changes later', () => {
  const run = buildRun({ config, usage: { completion_tokens: 3 }, env: {} });
  const env = buildPublishEnv({
    config: parseConfig(example), env: { ROSTER_MODEL: 'different-model' }, run,
  });
  assert.equal(env.AI_MODEL, 'owner/model');
  assert.equal(env.AI_CONTEXT_OUT, '3');
  assert.deepEqual(buildPublishEnv({ config, env: {}, run: null }), { AI_MODEL: 'owner/model' });
});

test('rejects invalid or overflowing returned counts and unsafe identifiers', () => {
  assert.throws(() => mergeUsage({ prompt_tokens: -1 }), /nonnegative safe integer/);
  assert.throws(() => mergeUsage({ completion_tokens: '12' }), /nonnegative safe integer/);
  assert.throws(() => mergeUsage({ prompt_tokens: Number.MAX_SAFE_INTEGER },
    { prompt_tokens: 1 }), /exceeds safe integer/);
  assert.throws(() => buildRun({ config, session: 'not a session' }), /AI_SESSION/);
  assert.throws(() => buildRun({
    config: parseConfig(example), env: { ROSTER_MODEL: 'unknown' },
  }), /unknown is not a model/);
  assert.throws(() => buildRun({ config, env: { AI_MODEL_VERSION: 'bad|version' } }), /version/);
});
