import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { buildRun, mergeUsage } from '../src/metrics/run.mjs';
import { packAgentRun, parseAgentRun } from '../vendor/github-agent-contracts/scripts/parse-agent-run.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
  .replace('model: ""', 'model: owner/model').replace('context_max: 0', 'context_max: 8192'));

test('builds a contracts-compatible compact AI-Run line from known token counts', () => {
  const usage = mergeUsage({ prompt_tokens: 12, completion_tokens: 5 },
    { prompt_tokens: 33, completion_tokens: 11 });
  const run = buildRun({ config, usage, session: 'roster-session', task: 'issue-42' });
  assert.equal(run.line, '1|-|owner/model@unknown|m|45/8192|16|roster-session|issue-42');
  assert.deepEqual(run.env, {
    AI_MODEL: 'owner/model', AI_EFFORT: 'm', AI_CONTEXT_MAX: '8192',
    AI_CONTEXT_USED: '45', AI_CONTEXT_OUT: '16',
    AI_SESSION: 'roster-session', AI_TASK: 'issue-42',
  });
  assert.equal(packAgentRun(run.env), run.line);
  assert.equal(parseAgentRun(run.line, config.llm.model).context_used, 45);
});

test('does not invent unknown model, provider, context, or token counts', () => {
  assert.equal(buildRun({ config: parseConfig(example), usage: {} }), null);
  const partial = mergeUsage({ prompt_tokens: 7 }, { prompt_tokens: 5, completion_tokens: 2 });
  assert.deepEqual(partial, { prompt_tokens: 12 });
  const run = buildRun({ config: parseConfig(example.replace('base_url: ""', 'base_url: http://localhost/v1')
    .replace('model: ""', 'model: m')), usage: partial });
  assert.equal(run.line, '1|-|m@unknown|m|12/-|-|-|-');
  assert.equal(run.env.AI_CONTEXT_OUT, undefined);
  assert.equal(run.env.AI_CONTEXT_MAX, undefined);
  assert.equal(run.env.AI_PROVIDER, undefined);
  assert.equal(packAgentRun(run.env), run.line);
  assert.deepEqual(mergeUsage(null, { prompt_tokens: 2 }), {});
});

test('records deterministic stub seats without inventing LLM usage', () => {
  const stub = buildRun({
    config: parseConfig(example), includeStub: true,
    session: 'roster-42-planner', task: 'issue-42',
  });
  assert.equal(stub.line, '1|-|builtin-stub@unknown|-|-/-|-|roster-42-planner|issue-42');
  assert.deepEqual(stub.env, {
    AI_MODEL: 'builtin-stub', AI_SESSION: 'roster-42-planner', AI_TASK: 'issue-42',
  });
  assert.equal(packAgentRun(stub.env), stub.line);
});

test('rejects invalid or overflowing returned counts and unsafe identifiers', () => {
  assert.throws(() => mergeUsage({ prompt_tokens: -1 }), /nonnegative safe integer/);
  assert.throws(() => mergeUsage({ completion_tokens: '12' }), /nonnegative safe integer/);
  assert.throws(() => mergeUsage({ prompt_tokens: Number.MAX_SAFE_INTEGER },
    { prompt_tokens: 1 }), /exceeds safe integer/);
  assert.throws(() => buildRun({ config, session: 'not a session' }), /AI_SESSION/);
});
