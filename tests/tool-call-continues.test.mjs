import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { captureCheckpoint } from '../src/lib/checkpoints.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runCoder } from '../src/seats/coder.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 8', 'turn_budget: 3'));

function fixture(context, task = planStub('Update `README.md` with a Status section.',
  { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-tool-call-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'),
    readFileSync(new URL('../principals/coder.md', import.meta.url), 'utf8'));
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nCode carefully.\n');
  writeFileSync(path.join(worktree, 'TASK.md'), task);
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  return { repoRoot, worktree, config, task: 'issue-4', session: 'roster-session',
    memoryPath: path.join(repoRoot, config.paths.memory) };
}

function response(finish_reason, message, usage) {
  return { status: 200, json: async () => ({
    choices: [{ finish_reason, message }], ...(usage ? { usage } : {}),
  }) };
}

function withResearch(fetchImpl) {
  return async (url, request) => {
    const body = request?.body ? JSON.parse(request.body) : null;
    if (body?.messages?.[0]?.content?.startsWith('You are the builtin research step.')) {
      return response('stop', { role: 'assistant', content: 'Inventory reviewed.' });
    }
    return fetchImpl(url, request);
  };
}

const writeCall = (content = '# Example\n\n## Status\nReady.\n') => ({
  id: 'write-1', type: 'function', function: {
    name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content }),
  },
});

test('a complete named-file write tool call continues into a summary and does not verdict tool_calls', async (context) => {
  const options = fixture(context, planStub('Update `README.md` with documentation.',
    { reference: 'issue:4', metadata: { task_class: 'docs', difficulty: 2 } }).task);
  let calls = 0;
  const events = [];
  const result = await runCoder({
    ...options, env: {}, onEvent: async (event) => events.push(event),
    runTestCommand: async () => ({ stdout: 'all tests pass', stderr: '' }),
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      if (calls === 1) return response('tool_calls',
        { role: 'assistant', content: null, tool_calls: [writeCall()] },
        { prompt_tokens: 10, completion_tokens: 4 });
      assert.equal(body.messages.at(-1).role, 'user');
      assert.match(body.messages.at(-1).content, /final summary/i);
      return response('stop', { role: 'assistant', content: 'Updated README Status.' },
        { prompt_tokens: 12, completion_tokens: 3 });
    },
  });
  assert.equal(result.error, undefined);
  assert.equal(result.excellence.pass, true);
  assert.equal(calls, 2);
  assert.equal(events.some((event) => event.type === 'seat-end' && event.verdict === 'tool_calls'), false);
  assert.match(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), /## Status/);
});

test('a docs draft disables thinking, uses at least 8192 completion tokens, and omits list_dir', async (context) => {
  const task = planStub('Update `README.md` with documentation.',
    { reference: 'issue:4', metadata: { task_class: 'docs', difficulty: 2 } }).task;
  const options = fixture(context, task);
  const bodies = [];
  await runCoder({
    ...options, env: {},
    runTestCommand: async () => ({ stdout: 'all tests pass', stderr: '' }),
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      bodies.push(body);
      assert.equal(body.chat_template_kwargs?.thinking, undefined);
      assert.equal(body.reasoning_effort, 'none');
      assert.ok(body.max_tokens >= 8192, body.max_tokens);
      assert.equal(body.tools.some((tool) => tool.function.name === 'list_dir'), false);
      return bodies.length === 1
        ? response('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] })
        : response('stop', { role: 'assistant', content: 'Updated README.' });
    },
  });
  assert.equal(bodies.length, 2);
});

test('a checkpoint failure does not emit a user-facing git-failure event', async (context) => {
  const unavailable = fixture(context);
  await assert.rejects(captureCheckpoint({ worktree: unavailable.worktree, task: unavailable.task,
    allowedFiles: ['README.md'], env: {} }), (error) => {
    assert.doesNotMatch(error.message, /git.?failure/i);
    assert.match(error.message, /no product restoration was authorized/);
    return true;
  });
  const options = fixture(context);
  execFileSync('git', ['init', '--quiet'], { cwd: options.worktree });
  execFileSync('git', ['add', '--', '.'], { cwd: options.worktree });
  execFileSync('git', ['-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
    'commit', '--quiet', '-m', 'baseline'], { cwd: options.worktree });
  const events = [];
  const result = await runCoder({
    ...options, env: {}, onEvent: async (event) => events.push(event),
    runTestCommand: async () => ({ stdout: 'all tests pass', stderr: '' }),
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      return body.messages.some((message) => message.role === 'tool')
        ? response('stop', { role: 'assistant', content: 'Updated README.' })
        : response('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] });
    },
  });
  assert.equal(result.excellence.pass, true);
  assert.equal(events.some((event) => /git.?failure/i.test(JSON.stringify(event))), false,
    JSON.stringify(events));
});

test('a response without usage leaves the context field unknown', async () => {
  const { createEventSink, createShellPainter } = await import('../src/shell/events.mjs');
  const { formatTray } = await import('../src/shell/tray.mjs');
  const { stripVTControlCharacters } = await import('node:util');
  const display = { contextUsed: undefined, contextMax: 1_000_000, issue: 4, seat: 'coder',
    state: 'drafting', model: 'local-model', startedAt: 0, busy: true };
  const sink = createEventSink({ emit: createShellPainter({ display }) });
  sink.receive({ type: 'seat-start', seat: 'coder' });
  sink.receive({ type: 'completion', reason: 'stop' });
  const rail = stripVTControlCharacters(formatTray(display, { columns: 120, color: false, now: 0 }).rail);
  assert.match(rail, /- \/ 1\.0m/);
});


test('an explicit recipe tool list restricts automatically offered tools without widening access', async (context) => {
  const task = planStub('Update `README.md` with documentation.',
    { reference: 'issue:4', metadata: { task_class: 'docs', difficulty: 2 } }).task;
  const options = fixture(context, task);
  let calls = 0;
  const result = await runCoder({ ...options, env: {},
    config: { ...config, seat: { ...config.seat, recipe_tools: ['write_file', 'web_search'] } },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      assert.deepEqual(body.tools.map((tool) => tool.function.name), calls === 0 ? ['write_file'] : []);
      return ++calls === 1
        ? response('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] })
        : response('stop', { role: 'assistant', content: 'Updated README.' });
    },
  });
  assert.equal(result.error, undefined);
  assert.equal(result.excellence.pass, true);
});

test('an explicitly empty recipe tool list denies even automatic write tools', async (context) => {
  const task = planStub('Update `README.md` with documentation.',
    { reference: 'issue:4', metadata: { task_class: 'docs', difficulty: 2 } }).task;
  const options = fixture(context, task);
  await assert.rejects(runCoder({ ...options, env: {},
    config: { ...config, seat: { ...config.seat, recipe_tools: [] } },
    fetchImpl: async (_url, request) => {
      assert.equal(Object.hasOwn(JSON.parse(request.body), 'tools'), false);
      return response('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] });
    },
  }), /invalid or unavailable tool/);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Example\n');
});

test('recipe tools deny research reads before implementation', async (context) => {
  const options = fixture(context);
  await assert.rejects(runCoder({ ...options, env: {},
    config: { ...config, seat: { ...config.seat, recipe_tools: [] } },
    fetchImpl() { assert.fail('denied research must not contact the LLM'); },
  }), /allow-list denies read_file/);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Example\n');
});
