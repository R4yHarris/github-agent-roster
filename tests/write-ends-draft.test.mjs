import assert from 'node:assert/strict';
import { cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { createShellPainter, createEventSink } from '../src/shell/events.mjs';
import { createTranscript } from '../src/shell/transcript.mjs';
import { formatTray } from '../src/shell/tray.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 8', 'turn_budget: 3'));

function fixture(context) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-write-draft-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'), '# Coder\n');
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\n');
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  const task = planStub('Update `README.md` with a Status section.', {
    reference: 'issue:4', metadata: { task_class: 'docs', difficulty: 4 },
  }).task;
  writeFileSync(path.join(worktree, 'TASK.md'), task);
  return { repoRoot, worktree, config, task: 'issue-4', session: 'test-session' };
}

function response(body) {
  return Response.json({ model: 'local-model', usage: { prompt_tokens: 10, completion_tokens: 4 },
    choices: [{ finish_reason: 'tool_calls', message: body }] });
}

test('a named-file write ends the draft as a write and proceeds to tests, not a tool_calls failure', async (context) => {
  const options = fixture(context);
  const events = [];
  let calls = 0;
  const result = await runCoder({
    ...options, env: {}, onEvent: async (event) => events.push(event),
    fetchImpl: async (_url, request) => {
      calls += 1;
      const sent = JSON.parse(request.body);
      if (calls === 1) {
        assert.equal(sent.stream, true);
        return response({ role: 'assistant', content: 'I wrote the file.', tool_calls: [{
          id: 'write', type: 'function', function: {
            name: 'write_file',
            arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
          },
        }] });
      }
      assert.equal(sent.messages.at(-1).content.includes('Task checks passed'), true);
      return Response.json({ model: 'local-model', usage: { prompt_tokens: 12, completion_tokens: 5 },
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Wrote README Status.' } }] });
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '', exit_code: 0 }),
  });
  assert.equal(result.excellence.pass, true);
  assert.equal(result.finishReason, undefined);
  assert.ok(events.some((event) => event.type === 'tool' && event.name === 'write_file'));
  assert.ok(events.some((event) => event.type === 'tool' && event.name === 'run_test'));
  assert.doesNotMatch(readFileSync(result.resultPath, 'utf8'), /tool_calls/);
});

test('content deltas append the new text without overlapping prior text', () => {
  const writes = [];
  const transcript = createTranscript({ color: false,
    write: (text, options = {}) => writes.push({ text: text.trimEnd(), replace: options.replace === true }) });
  const sink = createEventSink({ emit: createShellPainter({ transcript }) });
  sink.receive({ type: 'delta', text: "I'll add a" });
  sink.receive({ type: 'delta', text: ' new `## Status`' });
  assert.deepEqual(writes.map(({ text }) => text), ["I'll add a", ' new `## Status`']);
  assert.ok(writes.every(({ text, replace }) => replace === false && !text.includes('aThe') && !text.includes('#ME')));
});

test('repainting the same buffer does not insert its beginning into the middle', () => {
  const writes = [];
  const transcript = createTranscript({ color: false,
    write: (text, options = {}) => writes.push({ text: text.trimEnd(), replace: options.replace === true }) });
  const sink = createEventSink({ emit: createShellPainter({ transcript }) });
  sink.receive({ type: 'delta', text: "I'll add a new `## Status`" });
  sink.receive({ type: 'delta', text: "I'll add a new `## Status`" });
  assert.equal(writes.at(-1).text, "I'll add a new `## Status`");
  assert.doesNotMatch(writes.at(-1).text, /Status.*I'll/);
});

test('a stream without usage leaves the context field unknown', () => {
  const display = { issue: 4, seat: 'coder', state: 'drafting', model: 'local-model',
    contextUsed: undefined, contextMax: 1_000_000, startedAt: 0, busy: true };
  const sink = createEventSink({ emit: createShellPainter({ display }) });
  sink.receive({ type: 'seat-start', seat: 'coder' });
  const rail = formatTray(display, { columns: 120, color: false, now: 0 }).rail;
  assert.match(rail, /- \/ 1\.0m/);
});
