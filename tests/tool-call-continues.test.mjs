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

test('a hallucinated tool gets one correction and a reused gateway call id is re-minted', async (context) => {
  const options = fixture(context);
  const looser = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
    .replace('model: ""', 'model: local-model'));
  let calls = 0;
  const result = await runCoder({ ...options, config: looser, env: {}, fetchImpl: withResearch(async (_url, request) => {
    const body = JSON.parse(request.body);
    calls += 1;
    if (calls === 1) {
      return response('tool_calls', { role: 'assistant', content: null, tool_calls: [{ id: 'call_0', type: 'function',
        function: { name: 'read_file', arguments: JSON.stringify({ path: 'README.md' }) } }] });
    }
    if (calls === 2) {
      return response('tool_calls', { role: 'assistant', content: null, tool_calls: [{ id: 'call_1', type: 'function',
        function: { name: 'bash', arguments: JSON.stringify({ command: 'ls' }) } }] });
    }
    if (calls === 3) {
      assert.match(body.messages.at(-1).content, /Tool bash does not exist\. Offered tools: .*write_file/);
      return response('tool_calls', { role: 'assistant', content: null, tool_calls: [{ ...writeCall(), id: 'call_0' }] });
    }
    return response('stop', { role: 'assistant', content: 'Updated README.' });
  }) });
  assert.equal(result.error, undefined);
  assert.match(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), /## Status/);
});

test('a second hallucinated tool still fails the seat and names the tool', async (context) => {
  const options = fixture(context);
  await assert.rejects(runCoder({ ...options, env: {}, fetchImpl: withResearch(async () =>
    response('tool_calls', { role: 'assistant', content: null, tool_calls: [{ id: `x${Math.random()}`, type: 'function',
      function: { name: 'bash', arguments: '{}' } }] })) }), /invalid or unavailable tool: bash/);
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

test('an out-of-scope scratch file in the diff gets an in-loop correction and delete_file instead of failing the run', async (context) => {
  const options = fixture(context, planStub('Update `README.md` and `src/app.mjs` with a Status section and export.',
    { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task);
  const git = (...args) => execFileSync('git', args, { cwd: options.worktree, stdio: 'pipe' });
  git('init', '-q');
  git('add', '-A');
  git('-c', 'user.name=t', '-c', 'user.email=t@example.com', 'commit', '-qm', 'init');
  let calls = 0;
  const seen = [];
  const result = await runCoder({
    ...options, env: {},
    runTestCommand: async () => ({ stdout: 'all tests pass', stderr: '' }),
    fetchImpl: withResearch(async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      seen.push(body.messages.at(-1).content ?? '');
      if (calls === 1) {
        writeFileSync(path.join(options.worktree, 'probe.mjs'), 'console.log(1);\n');
        assert.ok(body.tools.some((tool) => tool.function.name === 'delete_file'));
        return response('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] });
      }
      if (seen.at(-1).includes('outside TASK.md Allowed Files')) {
        return response('tool_calls', { role: 'assistant', content: null, tool_calls: [{
          id: 'delete-1', type: 'function', function: { name: 'delete_file', arguments: JSON.stringify({ path: 'probe.mjs' }) },
        }] });
      }
      return response('stop', { role: 'assistant', content: 'Added the Status section; tests pass.' });
    }),
  });
  assert.equal(result.error, undefined, result.error?.message);
  assert.equal(result.excellence.pass, true, result.excellence.reasons.join('; '));
  assert.ok(seen.some((text) => text.includes('Diff path is outside TASK.md allowed paths: probe.mjs')));
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Example\n\n## Status\nReady.\n');
  assert.throws(() => readFileSync(path.join(options.worktree, 'probe.mjs')), /ENOENT/);
});

test('a full-suite regression outside Allowed Files is repaired, not excused as pre-existing', async (context) => {
  const options = fixture(context, planStub('Update `README.md` and `src/app.mjs` with a Status section and export.',
    { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task);
  mkdirSync(path.join(options.worktree, 'tests'));
  const consumer = path.join(options.worktree, 'tests', 'consumer.test.mjs');
  writeFileSync(consumer, "import test from 'node:test';\ntest('old status', () => {});\n");
  const git = (...args) => execFileSync('git', args, { cwd: options.worktree, stdio: 'pipe' });
  git('init', '-q');
  git('add', '-A');
  git('-c', 'user.name=t', '-c', 'user.email=t@example.com', 'commit', '-qm', 'init');
  const fixed = () => readFileSync(consumer, 'utf8').includes('new status');
  const failure = Object.assign(new Error('tests failed'), { code: 1,
    stdout: 'not ok 1 - old status\n', stderr: 'test at tests/consumer.test.mjs:2:1\n' });
  const runs = [];
  let calls = 0;
  const seen = [];
  const result = await runCoder({
    ...options, env: {},
    runTestCommand: async (program, args, { cwd }) => {
      runs.push([program === 'git' ? 'git' : 'node', path.resolve(cwd) === path.resolve(options.worktree) ? 'work' : 'base',
        ...args].join(' '));
      if (program === 'git' || path.resolve(cwd) !== path.resolve(options.worktree)) return { stdout: '', stderr: '' };
      if (args.includes('--test-timeout=120000') && !fixed()) throw failure;
      return { stdout: 'all tests pass', stderr: '' };
    },
    fetchImpl: withResearch(async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      seen.push(body.messages.at(-1).content ?? '');
      if (calls === 1) return response('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] });
      if (seen.at(-1).includes('so this change broke them: tests/consumer.test.mjs') && !fixed()) {
        return response('tool_calls', { role: 'assistant', content: null, tool_calls: [{
          id: `read-${calls}`, type: 'function', function: { name: 'read_file',
            arguments: JSON.stringify({ path: 'tests/consumer.test.mjs' }) } }, {
          id: `repair-${calls}`, type: 'function', function: { name: 'write_file', arguments: JSON.stringify({
            path: 'tests/consumer.test.mjs', content: "import test from 'node:test';\ntest('new status', () => {});\n" }) },
        }] });
      }
      return response('stop', { role: 'assistant', content: 'Added the Status section; tests pass.' });
    }),
  });
  assert.equal(result.error, undefined, result.error?.message);
  assert.equal(result.excellence.pass, true, result.excellence.reasons.join('; '));
  assert.ok(fixed(), 'the coder repaired the regressed consumer test');
  assert.ok(runs.some((run) => run.startsWith('git work worktree add --detach')), runs.join('\n'));
  assert.ok(runs.some((run) => run.startsWith('node base --test') && run.endsWith('tests/consumer.test.mjs')), runs.join('\n'));
  assert.ok(!seen.some((text) => text.includes('are outside Allowed Files and are pre-existing')));
});

test('a repaired own test no longer blocks steering when only a pre-existing outside test still fails', async (context) => {
  const options = fixture(context, planStub('Update `README.md`, `src/app.mjs` and `tests/app.test.mjs` with a Status section and export.',
    { reference: 'issue:5', metadata: { task_class: 'feat', difficulty: 4 } }).task);
  mkdirSync(path.join(options.worktree, 'tests'));
  const own = path.join(options.worktree, 'tests', 'app.test.mjs');
  writeFileSync(own, "import test from 'node:test';\ntest('broken', () => { throw new Error('x'); });\n");
  writeFileSync(path.join(options.worktree, 'tests', 'outside.test.mjs'), "import test from 'node:test';\ntest('o', () => {});\n");
  const ownFixed = () => readFileSync(own, 'utf8').includes('fixed');
  const passing = Array.from({ length: 400 }, (_, index) => `✔ passing test number ${index} (1.2ms)`).join('\n');
  const fail = (files) => Object.assign(new Error('tests failed'), { code: 1,
    stdout: `${passing}\n✖ failing tests:\n\n${files.map((file) => `test at ${file}:1:1\n✖ ${file} broke (2ms)\n  AssertionError: ${file} marker`).join('\n')}\n`,
    stderr: '' });
  let calls = 0;
  const seen = [];
  const result = await runCoder({
    ...options, env: {},
    runTestCommand: async (program, args) => {
      if (program === 'git') return { stdout: '', stderr: '' };
      // A pre-existing outside failure also fails when rerun alone; passing alone would make it transient.
      if (args.length === 3) throw fail([args[2]]);
      if (args.length !== 4) return { stdout: '', stderr: '' };
      throw fail(ownFixed() ? ['tests/outside.test.mjs'] : ['tests/app.test.mjs', 'tests/outside.test.mjs']);
    },
    fetchImpl: withResearch(async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      seen.push(body.messages.at(-1).content ?? '');
      if (calls === 1) return response('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] });
      if (seen.at(-1).includes('Repair 1 of') && !ownFixed()) {
        return response('tool_calls', { role: 'assistant', content: null, tool_calls: [{
          id: `read-${calls}`, type: 'function', function: { name: 'read_file',
            arguments: JSON.stringify({ path: 'tests/app.test.mjs' }) } }, {
          id: `repair-${calls}`, type: 'function', function: { name: 'write_file', arguments: JSON.stringify({
            path: 'tests/app.test.mjs', content: "import test from 'node:test';\ntest('fixed', () => {});\n" }) },
        }] });
      }
      return response('stop', { role: 'assistant', content: 'Added the Status section; own tests pass.' });
    }),
  });
  const repair = seen.find((text) => text.includes('Repair 1 of'));
  assert.ok(repair?.includes('AssertionError: tests/app.test.mjs marker'), 'repair evidence shows the failure, not passing lines');
  assert.ok(!repair.includes('passing test number 0 '), 'repair evidence skips the passing head');
  assert.ok(seen.some((text) => text.includes('are outside Allowed Files and are pre-existing: tests/outside.test.mjs')),
    seen.join('\n---\n'));
  assert.ok(!seen.some((text) => text.includes('Repair 2 of')), 'the cumulative repair grant is not mistaken for a current failure');
  assert.equal(result.progress?.testRepairs ?? 1, 1);
});
