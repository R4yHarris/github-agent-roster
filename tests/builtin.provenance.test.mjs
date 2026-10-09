import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';
import { fixture, llmConfig, stubConfig, runBuiltinIssue } from './helpers/builtin.mjs';

test('seat provenance retains requested/served models, observed times, route and response-backed usage', async (t) => {
  const options = fixture(t);
  const date = new Date('2026-10-09T00:00:00.000Z');
  let calls = 0;
  const result = await runBuiltinIssue(42, { ...options,
    config: { ...llmConfig, llm: { ...llmConfig.llm, fleet_profile: 'fixture-route', hardware: 'fixture-gpu' } },
    now: () => date, log: () => {},
    fetchImpl: async () => Response.json({ model: 'served-alias',
      usage: { prompt_tokens: 0, completion_tokens: 7 },
      choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{
        id: `bad-${++calls}`, type: 'function', function: { name: 'write_file', arguments: 'garbage' },
      }] } }] }),
  });
  assert.equal(result.failed, true);
  const { records } = await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll();
  const planner = records.find((record) => record.event === 'session');
  assert.equal(planner.requestedModel, 'local-model');
  assert.equal(planner.servedModel, 'served-alias');
  assert.deepEqual(planner.route, { name: 'fixture-route', profile: 'fixture-route', hardware: 'fixture-gpu' });
  assert.equal(planner.startedAt, date.toISOString());
  assert.equal(planner.endedAt, date.toISOString());
  assert.equal(planner.metrics.tokens_prompt, 0);
  assert.equal(planner.metrics.tokens_completion, 7);
  assert.equal(planner.metrics.cost_usd, 'unknown');
  assert.ok(planner.metrics.duration_ms >= 0);
  assert.equal(planner.outcome, '', 'planning evidence must not invent success');
});

test('stub seat evidence leaves models and usage unknown rather than borrowing environment values', async (t) => {
  const options = fixture(t);
  await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    env: { ...options.env, AI_CONTEXT_USED: '999', AI_CONTEXT_OUT: '888' },
  });
  const { records } = await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll();
  const planner = records.find((record) => record.sessionId === 'roster-42-planner');
  assert.equal(planner.requestedModel, '');
  assert.equal(planner.servedModel, '');
  assert.equal(planner.metrics.tokens_prompt, 'unknown');
  assert.equal(planner.metrics.tokens_completion, 'unknown');
});
