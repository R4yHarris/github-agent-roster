import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { humanEvalHint, recordedCoderRun } from '../src/lib/seat-publication.mjs';
import { materializeRun, buildPublishEnv } from '../src/metrics/run.mjs';

test('publication reads exact deepseek model and usage from the persisted coder row', (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-seat-publish-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  mkdirSync(join(repoRoot, '.roster', 'runs'), { recursive: true });
  const run = materializeRun({ provider: 'vllm', model: 'deepseek-v4.1-flash', effort: 'm',
    prompt_tokens: 100, completion_tokens: 40, context_max: 1048576,
    session: 'roster-92-coder', task: 'issue-92' });
  writeFileSync(join(repoRoot, '.roster', 'runs', 'runs.jsonl'), JSON.stringify(run.metrics) + '\n');
  const recorded = recordedCoderRun({ repoRoot, run });
  const env = buildPublishEnv({ config: { llm: {}, profiles: {}, publish: { enabled: true } },
    env: { AI_MODEL: 'GPT-6.1-Sol', AI_PROVIDER: 'github-copilot' }, run: recorded });
  assert.equal(env.AI_MODEL, 'deepseek-v4.1-flash');
  assert.equal(env.AI_CONTEXT_USED, '100');
  assert.equal(env.AI_CONTEXT_OUT, '40');
  assert.equal(humanEvalHint('roster-92-coder'), 'roster eval roster-92-coder accept 1 n --minutes M');
  writeFileSync(join(repoRoot, '.roster', 'runs', 'runs.jsonl'),
    JSON.stringify({ ...run.metrics, model: 'different-model' }) + '\n');
  assert.throws(() => recordedCoderRun({ repoRoot, run }), /differ from the reviewed run/);
});

test('missing journal evidence and invalid eval sessions fail explicitly', (t) => {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-seat-no-journal-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const run = materializeRun({ provider: 'local', model: 'served-model', effort: 'm',
    session: 'roster-92-coder', task: 'issue-92' });
  assert.throws(() => recordedCoderRun({ repoRoot, run }), /missing from/);
  assert.throws(() => humanEvalHint('ghcp-123'), /issue coder session/);
  assert.equal(humanEvalHint('roster-local-0123456789abcdef-coder'),
    'roster eval roster-local-0123456789abcdef-coder accept 1 n --minutes M');
  assert.throws(() => humanEvalHint('roster-local-bad-coder'), /local ask coder session/);
});
