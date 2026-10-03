import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { stripVTControlCharacters } from 'node:util';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runCoder as runCoderSeat } from '../src/seats/coder.mjs';
import { createEventSink, createShellPainter } from '../src/shell/events.mjs';
import { createTranscript } from '../src/shell/transcript.mjs';
import { formatTray } from '../src/shell/tray.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const llmConfig = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 8', 'turn_budget: 3'));

function fixture(context) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-stream-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'),
    readFileSync(new URL('../principals/coder.md', import.meta.url), 'utf8'));
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nCode carefully.\n');
  writeFileSync(path.join(worktree, 'TASK.md'),
    planStub('Update `README.md` with a Status section.',
      { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task);
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  return { repoRoot, worktree, config: llmConfig, task: 'issue-4', session: 'roster-session',
    memoryPath: path.join(repoRoot, llmConfig.paths.memory) };
}

function runCoder(options) {
  return runCoderSeat({ ...options, fetchImpl: withResearch(options.fetchImpl) });
}

// The research step is not a seat; it answers without usage so seat usage events stay unambiguous.
function withResearch(fetchImpl) {
  return async (url, request) => {
    const body = request?.body ? JSON.parse(request.body) : null;
    if (body?.messages?.[0]?.content?.startsWith('You are the builtin research step.')) {
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Inventory reviewed.' } }],
      }) };
    }
    return await fetchImpl(url, request);
  };
}

const streamHeaders = { 'Content-Type': 'text/event-stream' };
const frame = (chunk) => `data: ${JSON.stringify(chunk)}\n\n`;
const content = (text, extra = {}) => ({ model: 'local-model',
  choices: [{ index: 0, delta: { content: text }, ...extra }] });

function sse(chunks) {
  return new Response(`${chunks.map(frame).join('')}data: [DONE]\n\n`, { headers: streamHeaders });
}

function sseFromParts(parts) {
  const encoder = new TextEncoder();
  return new Response(new ReadableStream({
    async start(controller) {
      for (const part of parts) {
        if (typeof part === 'function') await part();
        else controller.enqueue(encoder.encode(part));
      }
      controller.close();
    },
  }), { headers: streamHeaders });
}

function failingStream(chunks, message) {
  const encoder = new TextEncoder();
  let sent = false;
  return new Response(new ReadableStream({
    pull(controller) {
      if (sent) {
        controller.error(new Error(message));
        return;
      }
      sent = true;
      controller.enqueue(encoder.encode(chunks.map(frame).join('')));
    },
  }), { headers: streamHeaders });
}

const toolCallChunk = (args, { name = 'write_file', id = 'write-1' } = {}) => ({ model: 'local-model',
  choices: [{ index: 0, delta: { tool_calls: [
    { index: 0, ...(id ? { id } : {}), type: 'function', function: { ...(name ? { name } : {}), arguments: args } },
  ] } }] });

const argumentChunk = (args) => ({ model: 'local-model', choices: [{ index: 0, delta: { tool_calls: [
  { index: 0, function: { arguments: args } },
] } }] });

function railFor(events, { contextMax = 1_000_000 } = {}) {
  const display = { issue: 4, seat: 'coder', state: 'drafting', model: 'local-model', effort: 'l',
    contextUsed: undefined, contextMax, startedAt: 0, busy: true };
  const sink = createEventSink({ emit: createShellPainter({ display }) });
  for (const event of events) sink.receive(event);
  return stripVTControlCharacters(formatTray(display, { columns: 120, color: false, now: 0 }).rail);
}

test('the draft and test seat calls request a stream and usage on the final chunk', async (context) => {
  const options = fixture(context);
  const bodies = [];
  const events = [];
  const result = await runCoder({
    ...options, env: {},
    onEvent: async (event) => { events.push(event); },
    runTestCommand: async () => ({ stdout: 'all tests pass', stderr: '' }),
    fetchImpl: async (_url, request) => {
      const sent = JSON.parse(request.body);
      bodies.push(sent);
      if (bodies.length === 1) {
        return sse([toolCallChunk(JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' })),
          { model: 'local-model', choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] }]);
      }
      return sse([content('Updated README Status; node --test passed.',
        { finish_reason: 'stop' })]);
    },
  });
  assert.equal(result.mode, 'llm');
  assert.ok(bodies.length >= 2, `expected at least two seat calls, saw ${bodies.length}`);
  for (const body of bodies) {
    assert.equal(body.stream, true);
    assert.deepEqual(body.stream_options, { include_usage: true });
  }
  assert.ok(events.some((event) => event.type === 'delta' && event.text));
});

test('content deltas reflow one paragraph instead of seeking across the window', () => {
  const writes = [];
  const transcript = createTranscript({ color: false, columns: () => 40,
    write: (text, writeOptions = {}) => writes.push({ text: text.trimEnd(), replace: writeOptions.replace === true }) });
  const sink = createEventSink({ emit: createShellPainter({ transcript }) });
  for (const text of ['Re', 'ad', 'me']) sink.receive({ type: 'delta', text });
  assert.deepEqual(writes.map((entry) => entry.text), ['Re', 'Read', 'Readme']);
  assert.deepEqual(writes.map((entry) => entry.replace), [false, true, true]);
});

test('a final usage chunk emits one usage event before the seat returns and lands on the rail', async (context) => {
  const options = fixture(context);
  const events = [];
  let returned = false;
  const result = await runCoder({
    ...options, env: {},
    onEvent: async (event) => {
      if (event.type === 'usage') assert.equal(returned, false, 'usage must arrive before the seat returns');
      events.push(event);
    },
    runTestCommand: async () => ({ stdout: 'all tests pass', stderr: '' }),
    fetchImpl: async (_url, request) => {
      const sent = JSON.parse(request.body);
      if (sent.tools && !sent.messages.some((message) => message.role === 'tool')) {
        return sse([toolCallChunk(JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' })),
          { model: 'local-model', choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] }]);
      }
      return sse([content('Updated README Status; node --test passed.', { finish_reason: 'stop' }),
        { model: 'local-model', choices: [],
          usage: { prompt_tokens: 12_400, completion_tokens: 80, total_tokens: 12_480 } }]);
    },
  });
  returned = true;
  assert.equal(result.mode, 'llm');
  const usageEvents = events.filter((event) => event.type === 'usage');
  assert.deepEqual(usageEvents, [{ type: 'usage', input: 12_400, output: 80 }]);
  assert.ok(railFor(usageEvents).includes('12.4k/1.0m'), railFor(usageEvents));
});

test('a tool call runs only after its argument JSON is complete', async (context) => {
  const options = fixture(context);
  const readme = path.join(options.worktree, 'README.md');
  const body = '# Example\n\n## Status\nReady.\n';
  const args = JSON.stringify({ path: 'README.md', content: body });
  const partial = args.slice(0, 18);
  let midStreamContent = null;
  const result = await runCoder({
    ...options, env: {},
    runTestCommand: async () => ({ stdout: 'all tests pass', stderr: '' }),
    fetchImpl: async (_url, request) => {
      const sent = JSON.parse(request.body);
      if (sent.tools && !sent.messages.some((message) => message.role === 'tool')) {
        return sseFromParts([
          frame(toolCallChunk(partial)),
          () => { midStreamContent = readFileSync(readme, 'utf8'); },
          frame(argumentChunk(args.slice(18))),
          frame({ model: 'local-model', choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] }),
          'data: [DONE]\n\n',
        ]);
      }
      return sse([content('Updated README Status; node --test passed.', { finish_reason: 'stop' })]);
    },
  });
  assert.equal(result.mode, 'llm');
  assert.equal(midStreamContent, '# Example\n', 'the tool must not run from a partial argument');
  assert.equal(readFileSync(readme, 'utf8'), body);
});

test('a stream error before any usage chunk fails the seat and leaves the context field unknown', async (context) => {
  const options = fixture(context);
  const events = [];
  await assert.rejects(runCoder({
    ...options, env: {},
    onEvent: async (event) => { events.push(event); },
    runTestCommand: async () => ({ stdout: 'all tests pass', stderr: '' }),
    fetchImpl: async () => failingStream([content('Upda')], 'socket closed mid-stream'),
  }), /stream/i);
  assert.deepEqual(events.filter((event) => event.type === 'usage'), []);
  assert.ok(railFor(events.filter((event) => event.type === 'usage')).includes('- / 1.0m'));
});
