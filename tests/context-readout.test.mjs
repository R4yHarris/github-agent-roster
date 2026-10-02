import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createRunLog } from '../src/lib/run-log.mjs';
import { createDebugLog } from '../src/lib/debug-log.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { formatContext } from '../src/shell/context.mjs';
import { formatHelp } from '../src/shell/commands.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

test('missing response usage remains unknown rather than borrowing config/env counts', () => {
  const output = formatContext({ seat: 'coder', provider: 'vllm', model: 'served', effort: 'low',
    priorFeedbackIncluded: false }, { packBudgetChars: 8000, env: { AI_CONTEXT_USED: '999' } });
  assert.match(output, /Input: -\nOutput: -\nContext max: -/);
  assert.match(output, /Pack budget \(characters\): 8000/);
  assert.match(output, /Prior feedback included: no/);
  assert.doesNotMatch(output, /999/);
  assert.match(formatHelp('context'), /provider, model, effort, input, output, context max, finish reason/);
  assert.match(formatHelp('context'), /Aliases: \/usage/);
});

test('measurements select metadata only and never send token counts into debug JSONL', async (t) => {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-context-readout-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const measured = [];
  const debug = createDebugLog({ env: {}, enabled: true, session: 'context-test' });
  const configured = { ...config, llm: { ...config.llm, base_url: 'http://localhost:8000/v1',
    model: 'requested-model', provider: 'vllm', context_max: 1048576 } };
  const logger = await createRunLog({ repoRoot: root, session: 'roster-108-coder', env: {}, debug,
    errorOutput: { write() {} }, observe: (event) => { if (event.type === 'seat-measurement') measured.push(event); } });
  await logger.seat('coder', 'roster-108-coder', configured, async (onEvent) => {
    await onEvent({ type: 'http', phase: 'start', effort: 'none' });
    await onEvent({ type: 'completion', reason: 'stop' });
    return { response: { model: 'actual-model', usage: { prompt_tokens: 17, completion_tokens: 9 } },
      packBudgetChars: 6000, priorFeedbackIncluded: true, summary: 'PRIVATE_COMPLETION_BODY' };
  });
  assert.equal(measured[0].model, 'actual-model');
  assert.equal(measured[0].effort, 'none');
  assert.equal(measured[0].input, 17);
  assert.equal(measured[0].output, 9);
  assert.equal(measured[0].contextMax, 1048576);
  assert.equal(measured[0].priorFeedbackIncluded, true);
  assert.doesNotMatch(JSON.stringify(measured), /PRIVATE_COMPLETION_BODY/);
  assert.doesNotMatch(readFileSync(debug.path, 'utf8'), /prompt_tokens|completion_tokens|"input"|PRIVATE_COMPLETION_BODY/);
});

test('context and usage are read-only aliases and retain the actual last seat after model selection', async () => {
  let text = '';
  const shell = createDispatcher({ config, env: {}, output: { write(value) { text += value; } },
    errorOutput: { write() {} }, services: { repositoryBranch: () => 'main' } });
  shell.state.lastMeasuredSeat = { seat: 'coder', provider: 'vllm', model: 'measured-model', effort: 'low',
    input: 17, output: 9, contextMax: 1048576, finishReason: 'stop', packBudgetChars: 6000, priorFeedbackIncluded: false };
  await shell.dispatch('/model different-session-model');
  text = '';
  await shell.dispatch('/context');
  const first = text;
  text = '';
  await shell.dispatch('/usage');
  assert.equal(text, first);
  assert.match(text, /Model: measured-model/);
  assert.doesNotMatch(text, /different-session-model/);
});
