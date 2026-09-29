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
  assert.deepEqual(run.env, {
    AI_PROVIDER: 'local', AI_MODEL: 'owner/model', AI_MODEL_VERSION: '-',
    AI_EFFORT: 'm', AI_CONTEXT_MAX: '8192',
    AI_CONTEXT_USED: '45', AI_CONTEXT_OUT: '16',
    AI_SESSION: 'roster-session', AI_TASK: 'issue-42',
  });
  assert.equal(packAgentRun(run.env), run.line);
  assert.equal(parseAgentRun(run.line, config.llm.model).context_used, 45);
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

test('omits AI-Run and inherited metadata when no model is configured', () => {
  const env = buildPublishEnv({
    config: parseConfig(example),
    env: { ...Object.fromEntries(RUN_ENV_NAMES.map((name) => [name, 'unknown'])),
      ROSTER_API_KEY: 'private-value', GITHUB_APP_ID: '123' },
  });
  assert.deepEqual(env, { GITHUB_APP_ID: '123' });
  assert.equal(packAgentRun(env), null);
});

test('publication uses config or ROSTER_MODEL and preserves known version without stale usage', () => {
  for (const configured of [true, false]) {
    const env = buildPublishEnv({
      config: configured ? config : parseConfig(example),
      env: { ROSTER_MODEL: 'served-model', AI_MODEL: 'unknown', AI_PROVIDER: 'vllm',
        AI_MODEL_VERSION: 'v2', AI_EFFORT: 'x', AI_CONTEXT_OUT: '999',
        AI_SESSION: 'roster-42-coder', AI_TASK: 'issue-42', ROSTER_API_KEY: 'private-value' },
    });
    assert.equal(env.AI_MODEL, configured ? 'owner/model' : 'served-model');
    assert.equal(env.AI_MODEL_VERSION, 'v2');
    assert.equal(env.AI_PROVIDER, 'local');
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
  assert.equal(buildPublishEnv({ config, env: {}, run: null }).AI_MODEL, undefined);
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
