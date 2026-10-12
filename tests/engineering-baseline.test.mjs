import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createBuiltinChat } from '../src/lib/llm.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:1234/v1').replace('model: ""', 'model: fixture'));

test('builtin requests carry one engineering baseline while preserving roles, tools, Ask and caller messages', async () => {
  const tools = [{ type: 'function', function: { name: 'read_file' } }];
  const messages = [{ role: 'system', content: 'Read-only critic. No writes or publication.' },
    { role: 'user', content: 'Preserve this exact Ask.' }];
  const original = structuredClone(messages);
  const seen = [];
  const chat = createBuiltinChat(config, { env: {}, fetchImpl: async (_url, request) => {
    const body = JSON.parse(request.body);
    seen.push(body);
    return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: '{}' } }] });
  } });
  await chat({ messages, tools });
  await chat({ messages: seen[0].messages, tools });
  assert.deepEqual(messages, original);
  for (const body of seen) {
    assert.equal(body.messages[0].content.split('# Engineering mission and operating baseline').length, 2);
    assert.ok(body.messages[0].content.startsWith(original[0].content));
    assert.equal(body.messages[1].content, original[1].content);
    assert.deepEqual(body.tools, tools);
    assert.match(body.messages[0].content, /TDD/);
    assert.match(body.messages[0].content, /isolated green cases do not establish interaction correctness/);
    assert.match(body.messages[0].content, /original input, not a logical item\/record index/);
    assert.match(body.messages[0].content, /Do not invent a quote-start versus EOF/);
    assert.match(body.messages[0].content, /No dependency is authorized/);
    assert.match(body.messages[0].content, /never invent/);
  }
});

test('length retry retains one baseline and original Ask without mutating the caller', async () => {
  const messages = [{ role: 'user', content: 'Exact retry Ask.' }];
  const original = structuredClone(messages);
  let calls = 0;
  const chat = createBuiltinChat(config, { env: {}, fetchImpl: async (_url, request) => {
    const body = JSON.parse(request.body);
    calls += 1;
    assert.equal(body.messages[0].role, 'system');
    assert.equal(body.messages[0].content.split('# Engineering mission and operating baseline').length, 2);
    assert.equal(body.messages[1].content, 'Exact retry Ask.');
    return Response.json({ choices: [{ finish_reason: calls === 1 ? 'length' : 'stop',
      message: { role: 'assistant', content: calls === 1 ? 'partial' : 'complete' } }] });
  } });
  const reply = await chat({ messages });
  assert.equal(calls, 2);
  assert.equal(reply.message.content, 'complete');
  assert.deepEqual(messages, original);
});

test('empty messages fail before any HTTP request', async () => {
  const chat = createBuiltinChat(config, { env: {}, fetchImpl: () => assert.fail('Invalid messages reached HTTP') });
  for (const messages of [undefined, null, [], {}]) {
    await assert.rejects(chat({ messages }), /nonempty chat messages/);
  }
});
