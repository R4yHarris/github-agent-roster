import assert from 'node:assert/strict';
import test from 'node:test';
import { stripVTControlCharacters } from 'node:util';
import { createChat } from '../src/llm/openai.mjs';
import { contextMaxForModel } from '../src/llm/window.mjs';
import { probeModelDetails } from '../src/onboard/wizard.mjs';
import { createEventSink, createShellPainter } from '../src/shell/events.mjs';
import { contextTone, formatContext, formatTray } from '../src/shell/tray.mjs';
import { formatUsage } from '../src/shell/usage.mjs';

const messages = [{ role: 'user', content: 'test-only-private-prompt' }];

function client(fetch, { llm = {}, onEvent } = {}) {
  return createChat({
    llm: { base_url: 'http://localhost:8000/v1', model: 'local-test-model', api_key_optional: true, ...llm },
  }, { fetch, env: {}, vault: { get: async () => undefined }, onEvent });
}

function sse(chunks) {
  return new Response(`${chunks.map((chunk) => `data: ${JSON.stringify(chunk)}`).join('\n\n')}\n\ndata: [DONE]\n\n`,
    { headers: { 'Content-Type': 'text/event-stream' } });
}

const deltaChunks = [
  { model: 'local-test-model', choices: [{ index: 0, delta: { role: 'assistant', content: 'hel' } }] },
  { model: 'local-test-model', choices: [{ index: 0, delta: { content: 'lo' }, finish_reason: 'stop' }] },
];

function railFor(events, { contextMax = 1_000_000, columns = 120 } = {}) {
  const display = { issue: 108, seat: 'coder', state: 'drafting', model: 'served-model', effort: 'l',
    contextUsed: undefined, contextMax, startedAt: 0, busy: true };
  const sink = createEventSink({ emit: createShellPainter({ display }) });
  for (const event of events) sink.receive(event);
  return { display, rail: stripVTControlCharacters(formatTray(display, { columns, color: false, now: 0 }).rail) };
}

test('a streaming usage chunk puts the server prompt tokens, a bar and a percent on the rail', async () => {
  const events = [];
  const chat = client(async () => sse([...deltaChunks,
    { model: 'local-test-model', choices: [],
      usage: { prompt_tokens: 12_400, completion_tokens: 80, total_tokens: 12_480 } }]),
  { onEvent: async (event) => { events.push(event); } });
  const response = await chat({ messages, stream: true });
  assert.equal(response.message.content, 'hello');
  assert.deepEqual(response.usage, { prompt_tokens: 12_400, completion_tokens: 80, total_tokens: 12_480 });
  const usageEvents = events.filter((event) => event.type === 'usage');
  assert.deepEqual(usageEvents, [{ type: 'usage', input: 12_400, output: 80 }]);
  const { rail } = railFor(usageEvents);
  assert.ok(rail.includes('12.4k/1.0m'), rail);
  assert.ok(rail.includes('1%'), rail);
  assert.ok(rail.includes('[----------]'), rail);
});

test('a streaming response without a usage chunk leaves the context field unknown', async () => {
  const events = [];
  const chat = client(async () => sse(deltaChunks), { onEvent: async (event) => { events.push(event); } });
  const response = await chat({ messages, stream: true });
  assert.equal(response.usage, null);
  assert.deepEqual(events.filter((event) => event.type === 'usage'), []);
  assert.equal(railFor([]).rail.includes('- / 1.0m'), true);
});

test('a non-streaming usage object reaches the rail as used over max', async () => {
  const events = [];
  const chat = client(async () => Response.json({
    model: 'local-test-model',
    choices: [{ message: { role: 'assistant', content: 'hello' }, finish_reason: 'stop' }],
    usage: { prompt_tokens: 500, completion_tokens: 20, total_tokens: 520 },
  }), { onEvent: async (event) => { events.push(event); } });
  await chat({ messages });
  const { rail } = railFor(events.filter((event) => event.type === 'usage'));
  assert.ok(rail.includes('500/1.0m'), rail);
});

test('a later call replaces the used figure rather than adding to it', async () => {
  const events = [];
  let calls = 0;
  const chat = client(async () => {
    calls += 1;
    return Response.json({
      model: 'local-test-model',
      choices: [{ message: { role: 'assistant', content: 'hello' }, finish_reason: 'stop' }],
      usage: { prompt_tokens: calls === 1 ? 500 : 900, completion_tokens: 20, total_tokens: 520 },
    });
  }, { onEvent: async (event) => { events.push(event); } });
  await chat({ messages });
  await chat({ messages });
  const { display, rail } = railFor(events.filter((event) => event.type === 'usage'));
  assert.equal(display.contextUsed, 900);
  assert.ok(rail.includes('900/1.0m'), rail);
  assert.equal(rail.includes('1.4k'), false);
});

test('a reported max_model_len becomes the denominator and a missing one keeps the configured max', async () => {
  const reported = await probeModelDetails('http://localhost:8000/v1', {
    fetchImpl: async () => Response.json({ data: [{ id: 'served-model', max_model_len: 131_072 }] }),
    env: {}, apiKeyEnv: 'ROSTER_API_KEY',
  });
  assert.equal(contextMaxForModel(reported, 'served-model', 1_000_000), 131_072);
  const silent = await probeModelDetails('http://localhost:8000/v1', {
    fetchImpl: async () => Response.json({ data: [{ id: 'served-model' }] }),
    env: {}, apiKeyEnv: 'ROSTER_API_KEY',
  });
  assert.equal(contextMaxForModel(silent, 'served-model', 1_000_000), 1_000_000);
  assert.equal(contextMaxForModel(reported, 'other-model', 1_000_000), 1_000_000);
  const { rail } = railFor([{ type: 'usage', input: 12_400, output: 80 }], { contextMax: 131_072 });
  assert.ok(rail.includes('12.4k/131.1k'), rail);
  assert.ok(rail.includes('9%'), rail);
});

test('occupancy thresholds report a tone rather than a terminal colour', () => {
  assert.deepEqual([49, 50, 80, 95].map((percent) => contextTone(percent * 10, 1000)),
    ['green', 'yellow', 'orange', 'red']);
  assert.equal(contextTone(0, 1000), 'green');
  assert.equal(contextTone(undefined, 1000), null);
});

test('the rail keeps the bar when wide, the percent when narrow and neither when tiny', () => {
  const usage = [{ type: 'usage', input: 124_000, output: 80 }];
  const wide = railFor(usage, { columns: 80 }).rail;
  assert.ok(wide.includes('[#'), wide);
  assert.ok(wide.includes('12%'), wide);
  const narrow = railFor(usage, { columns: 60 }).rail;
  assert.ok(narrow.includes('12%'), narrow);
  assert.equal(narrow.includes('['), false);
  const tiny = railFor(usage, { columns: 40 }).rail;
  assert.equal(tiny.includes('12%'), false);
  assert.equal(tiny.includes('124'), false);
  assert.ok(tiny.includes('served-model'), tiny);
  assert.ok(tiny.includes('0s'), tiny);
});

test('a streaming request asks the server for usage in the final chunk', async () => {
  let sent;
  const chat = client(async (url, options) => {
    sent = JSON.parse(options.body);
    return sse(deltaChunks);
  });
  await chat({ messages, stream: true });
  assert.equal(sent.stream, true);
  assert.deepEqual(sent.stream_options, { include_usage: true });
  let plain;
  const direct = client(async (url, options) => {
    plain = JSON.parse(options.body);
    return Response.json({ model: 'local-test-model',
      choices: [{ message: { role: 'assistant', content: 'hello' }, finish_reason: 'stop' }] });
  });
  await direct({ messages });
  assert.equal(plain.stream, false);
  assert.equal(plain.stream_options, undefined);
});

test('a long prompt with no server usage is never estimated from its length', async () => {
  const events = [];
  const chat = client(async () => Response.json({ model: 'local-test-model',
    choices: [{ message: { role: 'assistant', content: 'x'.repeat(5000) }, finish_reason: 'stop' }] }),
  { onEvent: async (event) => { events.push(event); } });
  await chat({ messages: [{ role: 'user', content: 'y'.repeat(40_000) }] });
  assert.deepEqual(events.filter((event) => event.type === 'usage'), []);
  const { display, rail } = railFor(events.filter((event) => event.type === 'usage'));
  assert.equal(display.contextUsed, undefined);
  assert.ok(rail.includes('- / 1.0m'), rail);
});

test('the shell probes the served window once a session and paints it as the denominator', async () => {
  const { createDispatcher } = await import('../src/repl.mjs');
  const { parseConfig } = await import('../src/lib/config.mjs');
  const { readFileSync } = await import('node:fs');
  const base = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));
  const config = { ...base, llm: { ...base.llm, base_url: 'http://localhost:8000/v1', model: 'served-model',
    context_max: 1_000_000 } };
  let probes = 0;
  const shell = createDispatcher({ cwd: process.cwd(), repoRoot: process.cwd(), config, env: {},
    output: { write() {} }, errorOutput: { write() {} },
    services: {
      repositoryRoot: () => process.cwd(),
      probeModelDetails: async () => {
        probes += 1;
        return [{ id: 'served-model', context_max: 131_072 }];
      },
      runBuiltinIssue: async () => ({ failed: false, command: null, issue: { number: 92 }, task: 'issue-92' }),
    } });
  assert.equal(shell.state.display.contextMax, 1_000_000);
  await shell.dispatch('/run 92');
  await new Promise((resolve) => { setImmediate(resolve); });
  assert.equal(shell.state.display.contextMax, 131_072);
  await shell.dispatch('/run 92');
  await new Promise((resolve) => { setImmediate(resolve); });
  assert.equal(probes, 1);
  assert.equal(shell.state.display.contextMax, 131_072);
});

test('a zero-price endpoint prints no cost figure on the rail or the usage panel', () => {
  const { display, rail } = railFor([{ type: 'usage', input: 12_400, output: 80 }]);
  assert.equal(rail.includes('$'), false);
  const panel = formatUsage(display, null, { now: 0 });
  assert.equal(panel.includes('$'), false);
  assert.match(panel, /Prompt tokens: 12400/);
});

test('the context field pairs used with max before the bar and the percent', () => {
  assert.equal(formatContext(12_400, 1_000_000, { color: false }), '12.4k/1.0m [----------] 1%');
  assert.equal(formatContext(124_000, 1_000_000, { color: false }), '124.0k/1.0m [#---------] 12%');
  assert.equal(formatContext(undefined, 1_000_000, { color: false }), '- / 1.0m');
  assert.equal(formatContext(12_400, 1_000_000, { color: false, detail: 'percent' }), '1%');
  assert.equal(formatContext(undefined, 1_000_000, { color: false, detail: 'percent' }), '-');
});
