import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { setTimeout as wait } from 'node:timers/promises';
import { stripVTControlCharacters } from 'node:util';
import { parseConfig } from '../src/lib/config.mjs';
import { createEventSink, createShellPainter } from '../src/shell/events.mjs';
import { createTranscript } from '../src/shell/transcript.mjs';
import { createTray, formatTray } from '../src/shell/tray.mjs';
import { formatUsage } from '../src/shell/usage.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

function sinkFixture({ coalesceMs = 200 } = {}) {
  const seen = [];
  let clock = 0;
  const sink = createEventSink({ emit: (event) => seen.push(event), issue: () => 108,
    clock: () => clock, coalesceMs });
  return { seen, sink, tick(ms) { clock += ms; } };
}

test('the sink records a phase, a collapsed tool, a rewritten wait and a verdict', () => {
  const { seen, sink, tick } = sinkFixture();
  sink.receive({ type: 'seat-start', seat: 'coder' });
  sink.receive({ type: 'tool', name: 'read_file', path: 'README.md' });
  tick(10);
  sink.receive({ type: 'tool', name: 'read_file', path: 'README.md' });
  tick(300);
  sink.receive({ type: 'tool', name: 'read_file', path: 'README.md' });
  sink.receive({ type: 'waiting', elapsedSeconds: 45, local: true });
  sink.receive({ type: 'waiting', elapsedSeconds: 46, local: true });
  sink.receive({ type: 'completion', reason: 'length' });
  sink.receive({ type: 'wrote', path: 'README.md' });
  sink.receive({ type: 'seat-end', seat: 'coder', verdict: 'fail' });
  assert.deepEqual(seen, [
    { kind: 'phase', issue: 108, phase: 'draft' },
    { kind: 'tool', name: 'read_file', target: 'README.md', count: 1 },
    { kind: 'tool', name: 'read_file', target: 'README.md', count: 3 },
    { kind: 'wait', seconds: 45, local: true },
    { kind: 'wait', seconds: 46, local: true },
    { kind: 'verdict', issue: 108, phase: 'draft', verdict: 'fail', reason: 'length', files: ['README.md'] },
  ]);
  const written = [];
  const transcript = createTranscript({ write: (text, options) => written.push([text.trimEnd(), options.replace]),
    color: false });
  for (const event of seen) createShellPainter({ transcript })(event);
  assert.deepEqual(written, [
    ['#108 draft', false],
    ['read_file README.md', false],
    ['read_file README.md \u00b7 3', true],
    ['waiting \u00b7 45s \u00b7 local hardware can take minutes after idle', false],
    ['waiting \u00b7 46s \u00b7 local hardware can take minutes after idle', true],
    ['#108 draft \u00b7 fail \u00b7 length \u00b7 README.md', false],
  ]);
});

test('a run_test tool announces the test phase and a seat error closes with no write', () => {
  const { seen, sink, tick } = sinkFixture();
  sink.receive({ type: 'seat-start', seat: 'reviewer' });
  tick(400);
  sink.receive({ type: 'tool', name: 'run_test', path: '.' });
  sink.receive({ type: 'seat-error', seat: 'reviewer' });
  assert.deepEqual(seen.map((event) => event.kind), ['phase', 'phase', 'tool', 'verdict']);
  assert.deepEqual(seen[1], { kind: 'phase', issue: 108, phase: 'test' });
  assert.equal(seen.at(-1).files, null);
  const written = [];
  createShellPainter({ transcript: createTranscript({
    write: (text) => written.push(text.trimEnd()), color: false }) })(seen.at(-1));
  assert.deepEqual(written, ['#108 test \u00b7 fail \u00b7 no write']);
});

test('usage updates the rail and /usage without printing a transcript line or an invented number', () => {
  const { seen, sink } = sinkFixture();
  sink.receive({ type: 'http', phase: 'start', seat: 'coder', thinking: false, maxTokens: 512 });
  sink.receive({ type: 'seat-measurement', seat: 'coder' });
  assert.deepEqual(seen.map((event) => event.kind), ['usage', 'usage']);
  const display = { model: 'deepseek-v4.1-flash', host: '192.168.1.48:8888', effort: 'none', startedAt: null };
  const written = [];
  const transcript = createTranscript({ write: (text) => written.push(text), color: false });
  const paint = createShellPainter({ transcript, display });
  for (const event of seen) paint(event);
  assert.deepEqual(written, []);
  assert.equal(display.contextUsed, undefined);
  assert.equal(display.thinking, false);
  assert.equal(display.maxTokens, 512);
  const panel = formatUsage(display, null);
  assert.match(panel, /Prompt tokens: -\nCompletion tokens: -\nContext max: -/);
  assert.match(panel, /Thinking: disabled\nMax completion tokens: 512/);
  assert.equal(stripVTControlCharacters(formatTray(display, { color: false }).rail).includes('- / -'), true);
  paint({ kind: 'usage', input: 524288, output: 40, contextMax: 1048576, finishReason: 'stop' });
  assert.match(stripVTControlCharacters(formatTray(display, { color: false, now: 0 }).rail),
    /\[#####-----\] 524\.3k \/ 1\.0m/);
  assert.match(formatUsage(display, null), /Prompt tokens: 524288\nCompletion tokens: 40/);
  assert.equal(written.length, 0);
});

test('a planner run through the log writes one line per event and no old sentences', async () => {
  const { mkdtempSync, rmSync } = await import('node:fs');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const { createRunLog } = await import('../src/lib/run-log.mjs');
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-one-writer-'));
  try {
    const written = [];
    let shell = '';
    const transcript = createTranscript({ write: (text) => written.push(text.trimEnd()), color: false });
    const sink = createEventSink({ emit: createShellPainter({ transcript }), issue: () => 108, clock: () => 0 });
    const logger = await createRunLog({ repoRoot, session: 'roster-108-planner', env: {},
      errorOutput: { write(text) { shell += text; } }, observe: (event) => sink.receive(event) });
    await logger.seat('planner', 'roster-108-planner', config, async (onEvent) => {
      await onEvent({ type: 'tool', name: 'write_file', path: 'TASK.md' });
      await onEvent({ type: 'wrote', path: 'TASK.md' });
      await onEvent({ type: 'waiting', host: 'localhost:8000', local: true, elapsedSeconds: 45 });
      await onEvent({ type: 'waiting', host: 'localhost:8000', local: true, elapsedSeconds: 46 });
      return { mode: 'stub', verdict: 'pass' };
    });
    sink.flush();
    assert.equal(shell, '');
    assert.deepEqual(written, [
      '#108 plan',
      'write_file TASK.md',
      'waiting \u00b7 45s \u00b7 local hardware can take minutes after idle',
      'waiting \u00b7 46s \u00b7 local hardware can take minutes after idle',
      '#108 plan \u00b7 pass \u00b7 TASK.md',
    ]);
    const log = readFileSync(join(repoRoot, '.roster', 'runs', 'roster-108-planner.log'), 'utf8');
    assert.match(log, /seat planner tool write_file path="TASK\.md"/);
    for (const sentence of ['Writing the plan', 'Saving TASK.md.', 'Still waiting on the model',
      'Drafting the change', 'Drafting at', 'before editing', 'Listing ']) {
      assert.ok(!shell.includes(sentence), sentence);
      assert.ok(!written.join('\n').includes(sentence), sentence);
    }
  } finally {
    rmSync(repoRoot, { recursive: true, force: true });
  }
});

test('debug prints the thinking flag and completion cap under the rail, never in the prompt', () => {
  const display = { issue: 108, state: 'drafting', model: 'deepseek-v4.1-flash', effort: 'l',
    startedAt: 0, thinking: false, maxTokens: 512 };
  const frame = formatTray(display, { columns: 80, color: false, now: 1000, debug: true });
  assert.equal(frame.detail, 'thinking off \u2502 max_tokens 512');
  assert.equal(frame.prompt, 'roster> ');
  assert.equal(formatTray(display, { columns: 80, color: false, now: 1000 }).detail, null);
  assert.equal(formatTray({ ...display, thinking: undefined, maxTokens: undefined },
    { color: false, debug: true }).detail, 'thinking - \u2502 max_tokens -');
});

test('a one-second rail tick repaints elapsed without reprinting the banner', async () => {
  const calls = [];
  let text = '';
  const output = { columns: 120, write(value) { text += String(value); }, flush() {} };
  const shell = { closed: false, pause() {}, resume() {}, setPrompt() {}, prompt() { calls.push('prompt'); },
    getCursorPos() { return { rows: 0 }; } };
  const state = { config, display: { issue: 108, state: 'drafting', model: 'deepseek-v4.1-flash', effort: 'l',
    busy: true, startedAt: Date.now() - 59000 }, pendingSecret: null, pendingQuestion: false,
  statusbar: true, debug: { enabled: false } };
  const tray = createTray({ output, state, shell, env: {}, cwd: process.cwd() });
  await tray.banner();
  tray.render();
  const before = calls.length;
  await wait(1200);
  tray.close();
  assert.ok(calls.length > before, 'the rail did not tick');
  assert.ok(calls.length - before <= 2, `the rail ticked ${calls.length - before} times in one second`);
  const plain = stripVTControlCharacters(text);
  assert.equal(plain.match(/^github-agent-roster {2}/gm)?.length, 1);
  assert.match(plain, /1m0[01]s/);
});
