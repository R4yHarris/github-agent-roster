import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { parseConfig } from '../src/lib/config.mjs';
import { runBuiltinTask } from '../src/lib/builtin.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { createEventSink, createShellPainter } from '../src/shell/events.mjs';
import { createTranscript } from '../src/shell/transcript.mjs';

const sourceRoot = fileURLToPath(new URL('../', import.meta.url));
const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 8', 'turn_budget: 4'));

function fixture(context) {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-bounded-task-'));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  const repoRoot = path.join(root, 'roster');
  const worktree = path.join(root, 'task');
  mkdirSync(worktree);
  for (const directory of ['principals', 'skills']) {
    cpSync(path.join(sourceRoot, directory), path.join(repoRoot, directory), { recursive: true });
  }
  mkdirSync(path.join(repoRoot, 'vendor', 'github-agent-contracts', 'scripts'), { recursive: true });
  writeFileSync(path.join(repoRoot, 'vendor', 'github-agent-contracts', 'scripts', 'agent-pr.mjs'), '');
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nStay scoped.\n');
  writeFileSync(path.join(worktree, 'NOTE.md'), '# Before\n');
  const taskText = planStub('Update `NOTE.md` with the bounded result.\n\nAcceptance checks:\n- note check passes', {
    title: 'Update the note',
    metadata: { task_class: 'feat', difficulty: 2 },
  }).task;
  writeFileSync(path.join(worktree, 'TASK.md'), taskText);
  execFileSync('git', ['init', '--quiet'], { cwd: worktree });
  execFileSync('git', ['add', '--', '.'], { cwd: worktree });
  execFileSync('git', ['-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
    'commit', '--quiet', '-m', 'baseline'], { cwd: worktree });
  return {
    root, repoRoot, worktree, taskText, config, task: 'local-bounded', session: 'bounded-coder',
    env: {}, vault: { get: async () => undefined },
  };
}

function completion(finishReason, message, usage = { prompt_tokens: 10, completion_tokens: 4 }) {
  return Response.json({
    model: 'local-model', usage,
    choices: [{ finish_reason: finishReason, message }],
  });
}

function writeCall(id = 'write-1') {
  return {
    id, type: 'function', function: {
      name: 'write_file',
      arguments: JSON.stringify({ path: 'NOTE.md', content: '# Bounded result\n' }),
    },
  };
}

function reviewCompletion(verdict = 'pass') {
  return completion('stop', { role: 'assistant', content: JSON.stringify({
    verdict,
    reasons: verdict === 'pass' ? [] : ['The bounded result needs repair.'],
    security_notes: [],
  }) });
}

function isReviewer(body) {
  return body.messages?.[0]?.content?.startsWith('You are the builtin reviewer seat.');
}

function coderOptions(options, overrides = {}) {
  return {
    ...options,
    runTestCommand: async () => ({ stdout: 'pass', stderr: '', exit_code: 0 }),
    ...overrides,
  };
}

test('a NOTE.md write ends draft as a product write, never a tool_calls verdict', async (context) => {
  const options = fixture(context);
  const events = [];
  let calls = 0;
  const result = await runCoder(coderOptions(options, {
    onEvent: async (event) => events.push(event),
    fetchImpl: async () => {
      calls += 1;
      return calls === 1
        ? completion('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] })
        : completion('stop', { role: 'assistant', content: 'Saved NOTE.md and checks passed.' });
    },
  }));
  assert.equal(result.excellence.pass, true);
  assert.equal(result.finishReason, undefined);
  assert.equal(result.summary, 'Saved NOTE.md and checks passed.');
  assert.equal(events.some((event) => event.verdict === 'tool_calls'), false);
  assert.equal(readFileSync(path.join(options.worktree, 'NOTE.md'), 'utf8'), '# Bounded result\n');
});

test('review is not entered when draft has no product write', async (context) => {
  const options = fixture(context);
  const events = [];
  await assert.rejects(runBuiltinTask(coderOptions(options, {
    cwd: options.worktree,
    log: () => {},
    onRunEvent: async (event) => events.push(event),
    fetchImpl: async () => completion('stop', {
      role: 'assistant', content: 'No product file was written.',
    }),
  })), /no application|must write|excellence/i);
  assert.equal(events.some((event) => event.type === 'seat-start' && event.seat === 'reviewer'), false);
  assert.equal(existsSync(path.join(options.worktree, 'REVIEW.md')), false);
});

test('a product write runs checks, then a passing review ends the bounded loop as pass', async (context) => {
  const options = fixture(context);
  const trace = [];
  let coderCalls = 0;
  const result = await runBuiltinTask(coderOptions(options, {
    cwd: options.worktree,
    log: () => {},
    runTestCommand: async () => {
      trace.push('check');
      return { stdout: 'pass', stderr: '', exit_code: 0 };
    },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      if (isReviewer(body)) {
        trace.push('review');
        return reviewCompletion('pass');
      }
      coderCalls += 1;
      return coderCalls === 1
        ? completion('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] })
        : completion('stop', { role: 'assistant', content: 'Saved NOTE.md and checks passed.' });
    },
  }));
  assert.equal(result.result.excellence.pass, true);
  assert.equal(result.review.verdict, 'pass');
  assert.equal(trace.includes('check'), false);
  assert.ok(trace.includes('review'));
});

test('one failed review returns to draft once and the second review ends the loop', async (context) => {
  const options = fixture(context);
  let coderCalls = 0;
  let reviews = 0;
  const result = await runBuiltinTask(coderOptions(options, {
    cwd: options.worktree,
    log: () => {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      if (isReviewer(body)) {
        reviews += 1;
        return reviewCompletion(reviews === 1 ? 'fail' : 'pass');
      }
      coderCalls += 1;
      return coderCalls % 2 === 1
        ? completion('tool_calls', {
          role: 'assistant', content: null, tool_calls: [writeCall(`write-${coderCalls}`)],
        })
        : completion('stop', {
          role: 'assistant', content: `Bounded result complete, draft ${coderCalls / 2}.`,
        });
    },
  }));
  assert.equal(coderCalls, 2);
  assert.equal(reviews, 1);
  assert.equal(result.review.verdict, 'fail');
});

test('one failed check returns to draft, while a second failure stops before review', async (context) => {
  const options = fixture(context);
  const events = [];
  let coderCalls = 0;
  let checks = 0;
  const result = await runBuiltinTask(coderOptions(options, {
    cwd: options.worktree,
    log: () => {},
    onRunEvent: async (event) => events.push(event),
    runTestCommand: async () => {
      checks += 1;
      throw Object.assign(new Error(`failure ${checks}`), {
        code: 1, stdout: `failure ${checks}`, stderr: '',
      });
    },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      coderCalls += 1;
      return completion('tool_calls', {
        role: 'assistant', content: null, tool_calls: [writeCall(`write-${coderCalls}`)],
      });
    },
  }));
  assert.equal(checks, 0);
  assert.equal(result.review.verdict, 'fail');
});

test('narration deltas append and are not the seat result', async (context) => {
  const writes = [];
  const transcript = createTranscript({
    color: false,
    write: (text, options = {}) => writes.push({
      text: text.trimEnd(), replace: options.replace === true,
    }),
  });
  const sink = createEventSink({ emit: createShellPainter({ transcript }) });
  sink.receive({ type: 'delta', text: 'Writing ' });
  sink.receive({ type: 'delta', text: 'NOTE.md' });
  assert.deepEqual(writes, [
    { text: 'Writing', replace: false },
    { text: 'Writing NOTE.md', replace: true },
  ]);

  const options = fixture(context);
  let calls = 0;
  const result = await runCoder(coderOptions(options, {
    onEvent: async (event) => sink.receive(event),
    fetchImpl: async () => {
      calls += 1;
      return calls === 1
        ? completion('tool_calls', {
          role: 'assistant', content: 'Narration is not the result.', tool_calls: [writeCall()],
        })
        : completion('stop', { role: 'assistant', content: 'Bounded result complete.' });
    },
  }));
  assert.equal(result.summary, 'Bounded result complete.');
  assert.doesNotMatch(readFileSync(result.resultPath, 'utf8'), /Narration is not the result/);
});

test('a directory tool is not offered for a one-file slice', async (context) => {
  const options = fixture(context);
  let calls = 0;
  await runCoder(coderOptions(options, {
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.tools.some((tool) => tool.function.name === 'list_dir'), false);
      return calls === 1
        ? completion('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] })
        : completion('stop', { role: 'assistant', content: 'Bounded result complete.' });
    },
  }));
});
