import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import { PassThrough } from 'node:stream';
import test from 'node:test';
import { stripVTControlCharacters } from 'node:util';
import { parseConfig } from '../src/lib/config.mjs';
import { createDispatcher, startRepl } from '../src/repl.mjs';
import { formatBanner, collectBannerFacts } from '../src/shell/banner.mjs';
import { createTray, formatTray, formatContext, formatContextBar, formatTokens } from '../src/shell/tray.mjs';
import { createTranscript } from '../src/shell/transcript.mjs';
import { formatUsage } from '../src/shell/usage.mjs';

const display = { issue: 108, branch: 'issue-108', seat: 'coder', state: 'drafting',
  model: 'deepseek-v4.1-flash', host: '192.168.1.48:8888', effort: 'l',
  contextUsed: undefined, contextMax: 1048576, startedAt: 0, busy: true };
const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

function fakeTerminal({ columns = 120, rows = 0 } = {}) {
  const calls = [];
  let text = '';
  const output = { columns,
    write(value) { text += String(value); calls.push('write'); },
    flush() { calls.push('flush'); } };
  const shell = { closed: false,
    pause() { calls.push('pause'); },
    resume() { calls.push('resume'); },
    setPrompt(value) { calls.push(`setPrompt:${stripVTControlCharacters(value)}`); },
    prompt(preserveCursor) { calls.push(`prompt:${preserveCursor}`); },
    getCursorPos() { return { rows }; } };
  return { calls, output, shell, get text() { return text; }, get plain() { return stripVTControlCharacters(text); } };
}

test('the pinned rail shows issue, the configured model, context counts and elapsed without a host or port', () => {
  const frame = formatTray(display, { columns: 80, now: 446000 });
  assert.equal(stripVTControlCharacters(frame.rail), '#108 draft \u2502 deepseek-v4.1-flash l \u2502 - / 1.0m \u2502 7m26s');
  assert.doesNotMatch(frame.rail, /192\.168|8888|localhost/);
  assert.equal(stripVTControlCharacters(frame.rule), '\u2500'.repeat(79));
  assert.equal(stripVTControlCharacters(frame.prompt), 'roster> ');
  const painted = `${frame.rule}\n${frame.rail}\n${frame.prompt}`;
  assert.match(painted, /\x1b\[96m/);
  assert.match(painted, /\x1b\[97m/);
  assert.doesNotMatch(painted, /\x1b\[34/);
  assert.doesNotMatch(painted, /\x1b\[2m/);
  assert.equal(stripVTControlCharacters(formatTray({ ...display, state: 'idle' }, { color: false }).rail),
    'idle \u2502 deepseek-v4.1-flash l \u2502 - / 1.0m \u2502 -');
  assert.match(formatTray(display, { debug: true, now: 446000 }).rail, /7m26s\x1b\[0m\x1b\[96m\*/);
});

test('a failed rail ends in the finish reason and drops fields from the right when narrow', () => {
  const failed = { ...display, state: 'failed', lastFinishReason: 'length' };
  const rail = formatTray(failed, { columns: 80, color: false, now: 446000 }).rail;
  assert.ok(rail.endsWith('length'), rail);
  assert.match(formatTray(failed, { columns: 80 }).rail, /\x1b\[91mlength/);
  const narrow = formatTray(failed, { columns: 30, color: false }).rail;
  assert.doesNotMatch(narrow, /1\.0m/);
  const tiny = formatTray(failed, { columns: 20, color: false }).rail;
  assert.equal(tiny, '#108 fail \u2502 length');
  for (const columns of [20, 30, 60, 120]) {
    assert.ok(stripVTControlCharacters(formatTray(failed, { columns }).rail).length < columns);
  }
});

test('the context bar is empty when usage is unknown and colours each occupancy threshold', () => {
  assert.equal(formatContextBar(undefined, 1000, { color: false }), '[----------]');
  assert.equal(formatContextBar(0, 0, { color: false }), '[----------]');
  assert.equal(formatContextBar(300, 1000, { color: false }), '[###-------]');
  assert.match(formatContextBar(300, 1000), /\x1b\[92m/);
  assert.match(formatContextBar(600, 1000), /\x1b\[93m/);
  assert.match(formatContextBar(850, 1000), /\x1b\[38;5;208m/);
  assert.match(formatContextBar(990, 1000), /\x1b\[91m/);
  assert.deepEqual([0, 999, 5200, 1048576, undefined].map(formatTokens), ['0', '999', '5.2k', '1.0m', '-']);
  assert.equal(formatContext(undefined, 1048576, { color: false }), '- / 1.0m');
  assert.equal(formatContext(5200, 1048576, { color: false }), '[#---------] 5.2k / 1.0m');
});

test('the banner prints once with every fact and keeps an unknown fact as a dash', async () => {
  const facts = await collectBannerFacts({ env: {}, cwd: 'D:\\oss\\github-agent-roster', branch: undefined,
    llm: { model: 'deepseek-v4.1-flash', base_url: 'http://192.168.1.48:8888/v1' } });
  assert.equal(facts.branch, '-');
  assert.equal(facts.update, 'skipped');
  assert.equal(facts.endpoint, 'configured');
  assert.match(facts.contracts, /^v?\d+\.\d+\.\d+/);
  const plain = stripVTControlCharacters(formatBanner(facts));
  assert.match(plain, /^github-agent-roster {2}\S/);
  assert.doesNotMatch(plain, /^github-agent-roster {2}0\.0\.0/);
  assert.match(plain, /node \d+ \u00b7 .+ \u00b7 D:\\oss\\github-agent-roster \u00b7 -/);
  assert.match(plain, /endpoint configured \u00b7 deepseek-v4\.1-flash \u00b7 192\.168\.1\.48:8888/);
  assert.match(plain, /update skipped \u00b7 warnings none/);
  const broken = await collectBannerFacts({ env: {}, cwd: process.cwd(), branch: 'main', llm: {},
    services: { latestRelease: () => { throw new Error('offline'); } } });
  assert.equal(broken.warnings.length, 1);
  assert.match(stripVTControlCharacters(formatBanner(broken)), /warnings 1\nupdate check failed: offline/);
});

test('a repeated read collapses into one counted transcript line rewritten in place', () => {
  const written = [];
  const transcript = createTranscript({ write: (text, options) => written.push([text, options]), color: false });
  transcript.phase(108, 'draft');
  transcript.tool('read', 'README.md');
  transcript.tool('read', 'README.md');
  transcript.tool('read', 'README.md');
  transcript.waiting(['45s', 'local hardware can take minutes after idle']);
  transcript.waiting(['46s', 'local hardware can take minutes after idle']);
  transcript.verdict(['#108 review', 'fail', 'length']);
  assert.deepEqual(written.map(([text]) => text.trimEnd()), [
    '#108 draft',
    'read README.md',
    'read README.md \u00b7 2',
    'read README.md \u00b7 3',
    'waiting \u00b7 45s \u00b7 local hardware can take minutes after idle',
    'waiting \u00b7 46s \u00b7 local hardware can take minutes after idle',
    '#108 review \u00b7 fail \u00b7 length',
  ]);
  assert.deepEqual(written.map(([, options]) => options.replace), [false, false, true, true, false, true, false]);
});

test('seat and test events update the cached display without running another seat', async () => {
  const updates = [];
  let messages = '';
  const shell = createDispatcher({ config, cwd: process.cwd(), env: {},
    output: { write(text) { messages += text; } },
    errorOutput: { write() {} }, onStateChange: (state) => updates.push({ ...state.display }),
    services: { repositoryBranch: () => 'main', runBuiltinIssue: async (issue, options) => {
      await options.onRunEvent({ type: 'seat-start', seat: 'coder', model: 'served', host: 'localhost',
        effort: 'l', contextMax: 1048576 });
      await options.onRunEvent({ type: 'tool', name: 'run_test', seat: 'coder' });
      await options.onRunEvent({ type: 'seat-start', seat: 'reviewer', effort: 'l' });
      return { issue: { number: Number(issue) }, task: `issue-${issue}`, review: { verdict: 'fail' } };
    } } });
  await shell.dispatch('/run 108');
  assert.ok(updates.some(({ state, seat }) => state === 'drafting' && seat === 'coder'));
  assert.ok(updates.some(({ state }) => state === 'testing'));
  assert.ok(updates.some(({ state }) => state === 'reviewing'));
  assert.equal(shell.state.display.state, 'failed');
  assert.equal(shell.state.display.branch, 'issue-108');
  await shell.dispatch('/usage');
  assert.match(messages, /Model: served\nEndpoint: localhost/);
  assert.match(messages, /Prompt tokens: -\nCompletion tokens: -/);
  assert.match(messages, /Tool calls: 0\n/);
  await shell.dispatch('/statusbar off');
  assert.equal(shell.state.statusbar, false);
  assert.match(messages, /Status bars off\.\n/);
  assert.doesNotMatch(messages, /\x1b\[/);
  await shell.dispatch('/statusbar on');
  assert.equal(shell.state.statusbar, true);
});

test('TTY shell prints the banner once above the pinned rail and bright cyan input', async () => {
  const input = new PassThrough();
  input.isTTY = true;
  input.setRawMode = () => {};
  const output = new PassThrough();
  output.isTTY = true;
  output.columns = 120;
  let text = '';
  output.on('data', (data) => { text += data.toString(); });
  const done = startRepl({ input, output, errorOutput: { write() {} }, config, env: {},
    services: { repositoryRoot: () => process.cwd(), repositoryBranch: () => 'main' } });
  input.write('/quit\n');
  assert.equal(await done, 0);
  const plain = stripVTControlCharacters(text);
  assert.equal(plain.match(/^github-agent-roster {2}/gm)?.length, 1);
  assert.match(plain, /update skipped \u00b7 warnings/);
  assert.match(plain, /idle \u2502 .+ \u2502 - \/ /);
  assert.match(plain, /\u2500{40}/);
  assert.match(plain, /roster> /);
  assert.doesNotMatch(text, /\x1b\[34/);
  assert.doesNotMatch(text, /\x1b\[2m/);
  input.destroy();
  output.destroy();
});

test('redraw repaints elapsed without reprinting the banner or clearing scrollback', async () => {
  const terminal = fakeTerminal();
  const state = { display: { ...display, startedAt: Date.now() }, config,
    pendingSecret: null, pendingQuestion: false, statusbar: true, debug: { enabled: false } };
  const tray = createTray({ ...terminal, state, env: {}, cwd: process.cwd() });
  await tray.banner();
  await tray.banner();
  tray.render();
  state.display.startedAt = Date.now() - 130000;
  tray.render();
  assert.equal(terminal.plain.match(/^github-agent-roster {2}/gm)?.length, 1);
  assert.match(terminal.plain, /2m10s/);
  assert.ok(!terminal.text.includes('\x1b[2J'));
  tray.close();
});

test('a rail tick flushes and prompts without waiting for input', (t) => {
  t.mock.timers.enable(['setInterval']);
  const terminal = fakeTerminal();
  const state = { display: { ...display, startedAt: Date.now() }, config,
    pendingSecret: null, pendingQuestion: false, statusbar: true, debug: { enabled: false } };
  const tray = createTray({ ...terminal, state, env: {}, cwd: process.cwd() });
  tray.render();
  terminal.calls.length = 0;
  t.mock.timers.tick(1000);
  assert.ok(terminal.calls.includes('flush'));
  assert.ok(terminal.calls.includes('prompt:true'));
  const firstTickCalls = terminal.calls.length;
  t.mock.timers.tick(1000);
  assert.ok(terminal.calls.length > firstTickCalls);
  assert.ok(terminal.calls.slice(firstTickCalls).includes('flush'));
  assert.ok(terminal.calls.slice(firstTickCalls).includes('prompt:true'));
  tray.close();
});

test('the writer pauses, flushes and repaints the rail around transcript output', () => {
  const terminal = fakeTerminal();
  const state = { display: { ...display, startedAt: Date.now() - 130000 }, config,
    pendingSecret: null, pendingQuestion: false, statusbar: true, debug: { enabled: false } };
  const tray = createTray({ ...terminal, state, env: {}, cwd: process.cwd() });
  tray.write('read README.md\n');
  assert.match(terminal.plain, /read README\.md/);
  assert.match(terminal.plain, /#108 draft \u2502 deepseek-v4\.1-flash l/);
  assert.match(terminal.plain, /2m10s/);
  assert.deepEqual(terminal.calls.slice(0, 3), ['pause', 'write', 'flush']);
  assert.ok(terminal.calls.includes('setPrompt:roster> '));
  assert.ok(terminal.calls.includes('prompt:true'));
  assert.equal(terminal.calls.at(-1), 'flush');
  state.statusbar = false;
  terminal.calls.length = 0;
  tray.write('read README.md\n');
  assert.doesNotMatch(stripVTControlCharacters(terminal.text.split('read README.md')[2] ?? ''), /\u2500{10}/);
  assert.equal(formatUsage({}, null).includes('Model: -'), true);
  tray.close();
});

test('a transcript append without a newline still prompts after flushing', () => {
  const terminal = fakeTerminal();
  const state = { display: { ...display, startedAt: Date.now() - 130000 }, config,
    pendingSecret: null, pendingQuestion: false, statusbar: true, debug: { enabled: false } };
  const tray = createTray({ ...terminal, state, env: {}, cwd: process.cwd() });
  tray.write('partial transcript');
  assert.ok(terminal.calls.includes('flush'));
  assert.ok(terminal.calls.includes('prompt:true'));
  assert.equal(terminal.calls.at(-1), 'flush');
  tray.close();
});

function fakeScreen({ columns = 120 } = {}) {
  const rows = [''];
  let row = 0;
  let col = 0;
  const put = (text) => {
    for (const character of text) {
      while (rows.length <= row) rows.push('');
      if (character === '\n') { row += 1; col = 0; while (rows.length <= row) rows.push(''); continue; }
      if (character === '\r') { col = 0; continue; }
      const line = rows[row].padEnd(col, ' ');
      rows[row] = line.slice(0, col) + character + line.slice(col + 1);
      col += 1;
    }
  };
  const write = (value) => {
    let rest = String(value);
    while (rest.length > 0) {
      const index = rest.indexOf('\x1b[');
      if (index === -1) { put(rest); return; }
      put(rest.slice(0, index));
      const match = /^\x1b\[([0-9;]*)([A-Za-z])/.exec(rest.slice(index));
      if (match === null) { put('\x1b'); rest = rest.slice(index + 1); continue; }
      const [sequence, args, kind] = match;
      const count = Number.parseInt(args, 10);
      if (kind === 'A') row = Math.max(0, row - (count || 1));
      else if (kind === 'B') row += count || 1;
      else if (kind === 'G') col = Math.max(0, (count || 1) - 1);
      else if (kind === 'H') { row = 0; col = 0; }
      else if (kind === 'K') rows[row] = args === '2' ? '' : rows[row].slice(0, col);
      else if (kind === 'J') { rows[row] = rows[row].slice(0, col); rows.length = row + 1; }
      rest = rest.slice(index + sequence.length);
    }
  };
  return { rows, write, columns, get lines() { return rows.map((line) => line.trimEnd()); } };
}

function fakeReadline(screen) {
  let promptText = 'roster> ';
  let line = '';
  const shell = { closed: false, pause() {}, resume() {},
    setPrompt(value) { promptText = value; },
    prompt() { screen.write(`\x1b[1G\x1b[2K${promptText}${line}`); },
    getCursorPos() { return { rows: 0, cols: stripVTControlCharacters(promptText + line).length } } };
  return { shell, type(text) { line = text; shell.prompt(true); }, submit() { line = ''; screen.write('\n'); } };
}

test('a rail redraw repaints above the input row and never joins the rule to a typed command', () => {
  const screen = fakeScreen();
  const output = { columns: screen.columns, write: (value) => screen.write(value), flush() {} };
  const { shell, type } = fakeReadline(screen);
  const state = { display: { ...display, startedAt: Date.now() - 130000 }, config,
    pendingSecret: null, pendingQuestion: false, statusbar: true, debug: { enabled: true } };
  const tray = createTray({ output, shell, state, env: {}, cwd: process.cwd() });
  tray.render();
  type('/run 108');
  tray.render();
  tray.render();
  const lines = screen.lines;
  assert.equal(lines.filter((line) => line.includes('/run 108')).length, 1);
  assert.equal(lines.find((line) => line.includes('/run 108')), 'roster> /run 108');
  assert.doesNotMatch(screen.lines.join('\n'), /\u2500.*\/run 108|\/run 108.*\u2500/);
  assert.equal(lines.filter((line) => line.startsWith('\u2500')).length, 2);
  assert.equal(lines.filter((line) => line.includes('#108 draft')).length, 1);
  assert.equal(lines.at(-1), 'roster> /run 108');
  assert.ok(!screen.rows.join('\n').includes('\x1b[34'));
  assert.ok(!screen.rows.join('\n').includes('\x1b[2m'));
  tray.close();
});

test('a rewritten waiting tick never consumes a line the user already submitted', () => {
  const screen = fakeScreen();
  const output = { columns: screen.columns, write: (value) => screen.write(value), flush() {} };
  const { shell, type, submit } = fakeReadline(screen);
  const state = { display: { ...display, startedAt: Date.now() - 130000 }, config,
    pendingSecret: null, pendingQuestion: false, statusbar: true, debug: { enabled: false } };
  const tray = createTray({ output, shell, state, env: {}, cwd: process.cwd() });
  tray.render();
  type('/debug on');
  submit();
  tray.committed();
  tray.write('waiting \u00b7 46s\n', output, { replace: true });
  const lines = screen.lines;
  assert.equal(lines.filter((line) => line.includes('/debug on')).length, 1);
  assert.match(lines.join('\n'), /waiting \u00b7 46s/);
  tray.close();
});

test('a resize clears from the banner down and later ticks keep one rail at the new width', async (t) => {
  t.mock.timers.enable(['setInterval']);
  const screen = fakeScreen({ columns: 40 });
  const output = Object.assign(new EventEmitter(), {
    columns: 40,
    write: (value) => screen.write(value),
    flush() {},
  });
  const { shell } = fakeReadline(screen);
  const state = { display: { ...display, startedAt: Date.now() - 130000 }, config,
    pendingSecret: null, pendingQuestion: false, statusbar: true, debug: { enabled: false } };
  const tray = createTray({ output, shell, state, env: {}, cwd: process.cwd() });
  await tray.banner();
  tray.render();
  assert.equal(screen.lines.filter((line) => line === '\u2500'.repeat(39)).length, 2);

  output.columns = 80;
  output.emit('resize');
  assert.equal(screen.lines.filter((line) => line === '\u2500'.repeat(39)).length, 0);
  assert.equal(screen.lines.filter((line) => line === '\u2500'.repeat(79)).length, 2);
  assert.equal(screen.lines.filter((line) => line.startsWith('github-agent-roster')).length, 1);

  t.mock.timers.tick(1000);
  assert.equal(screen.lines.filter((line) => line === '\u2500'.repeat(39)).length, 0);
  assert.equal(screen.lines.filter((line) => line === '\u2500'.repeat(79)).length, 2);
  tray.close();
});
