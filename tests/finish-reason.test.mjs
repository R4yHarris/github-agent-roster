import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createBuiltinChat } from '../src/lib/llm.mjs';
import { createRunLog } from '../src/lib/run-log.mjs';
import { UnsupportedFinishReasonError } from '../src/llm/finish-reason.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runLoop } from '../src/runtime/loop.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: local-model'));
const completion = (reason, content = 'Done.', usage) => Response.json({
  model: 'served-model', choices: [{ finish_reason: reason, message: { role: 'assistant', content } }], usage,
});

test('length gets exactly one retry with a smaller cap and no truncated body replay', async () => {
  const requests = [];
  const events = [];
  const chat = createBuiltinChat({ ...config, llm: { ...config.llm, max_tokens: 2048 } }, {
    env: {}, onEvent: (event) => events.push(event),
    fetchImpl: async (_url, request) => {
      requests.push(JSON.parse(request.body));
      return requests.length === 1
        ? completion('length', 'PRIVATE_TRUNCATED_BODY', { prompt_tokens: 10, completion_tokens: 20 })
        : completion('stop', 'Done.', { prompt_tokens: 3, completion_tokens: 2 });
    },
  });
  const response = await chat({ messages: [{ role: 'user', content: 'Implement the task.' }] });
  assert.deepEqual(requests.map(({ max_tokens }) => max_tokens), [2048, 1024]);
  assert.doesNotMatch(JSON.stringify(requests[1]), /PRIVATE_TRUNCATED_BODY/);
  assert.equal(response.finish_reason, 'stop');
  assert.equal(chat.lastAttempts, 2);
  assert.deepEqual(response.usage, { prompt_tokens: 13, completion_tokens: 22 });
  assert.deepEqual(chat.lastResponse, { model: 'served-model', usage: { prompt_tokens: 3, completion_tokens: 2 } });
  assert.deepEqual(events.filter(({ type }) => type === 'finish-reason'),
    [{ type: 'finish-reason', reason: 'length', retry: true }]);
});

test('a second length response fails by name and cannot consume another retry', async () => {
  let calls = 0;
  const chat = createBuiltinChat(config, { env: {}, fetchImpl: async () => {
    calls += 1;
    return completion('length', 'PRIVATE_RESPONSE_BODY');
  } });
  await assert.rejects(chat({ messages: [{ role: 'user', content: 'Task' }] }), (error) =>
    error instanceof UnsupportedFinishReasonError && /finish reason: length/.test(error.message) &&
    !error.message.includes('PRIVATE_RESPONSE_BODY'));
  assert.equal(calls, 2);
});

test('the length retry budget remains spent on later requests in the same coder chat', async () => {
  let calls = 0;
  const chat = createBuiltinChat(config, { env: {}, fetchImpl: async () => {
    calls += 1;
    return completion(calls === 2 ? 'stop' : 'length');
  } });
  await chat({ messages: [{ role: 'user', content: 'Task' }] });
  await assert.rejects(chat({ messages: [{ role: 'user', content: 'Continue' }] }), /finish reason: length/);
  assert.equal(calls, 3);
});

test('unknown reasons are named in shell and run log without response bodies', async (t) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-finish-log-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  let shell = '';
  const logger = await createRunLog({ repoRoot, session: 'roster-42-coder', env: {},
    errorOutput: { write(text) { shell += text; } } });
  await assert.rejects(logger.seat('coder', 'roster-42-coder', config, async (onEvent) => {
    const chat = createBuiltinChat(config, { env: {}, onEvent,
      fetchImpl: async () => completion('content_filter', 'PRIVATE_RESPONSE_BODY') });
    return chat({ messages: [{ role: 'user', content: 'Task' }] });
  }), /finish reason: content_filter/);
  assert.match(shell, /Unsupported LLM finish reason: content_filter/);
  const log = readFileSync(logger.path, 'utf8');
  assert.match(log, /finish_reason="content_filter" retry=false/);
  assert.doesNotMatch(shell + log, /PRIVATE_RESPONSE_BODY/);
});

test('length retry is logged in both projections and counted as a coder repair turn', async (t) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-length-log-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  let shell = '';
  let calls = 0;
  const logger = await createRunLog({ repoRoot, session: 'roster-42-coder', env: {},
    errorOutput: { write(text) { shell += text; } } });
  const task = planStub('Update README.md.').task;
  const result = await logger.seat('coder', 'roster-42-coder', config, (onEvent) => runLoop({
    config: { ...config, seat: { ...config.seat, turn_budget: 1 } },
    context: { task, pack: task }, env: {}, onEvent,
    tools: { run_test: async () => ({ exit_code: 0, stdout: 'pass', stderr: '' }) },
    fetchImpl: async () => completion(++calls === 1 ? 'length' : 'stop', 'PRIVATE_BODY'),
    verify: () => ({ pass: true, reasons: [] }),
  }));
  assert.equal(result.error, undefined);
  assert.equal(result.turns, 2);
  assert.match(shell, /Response truncated\. Retrying\./);
  assert.match(readFileSync(logger.path, 'utf8'), /finish_reason="length" retry=true Response truncated\. Retrying\./);
  assert.doesNotMatch(shell + readFileSync(logger.path, 'utf8'), /PRIVATE_BODY/);
});
