import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { PassThrough } from 'node:stream';
import test from 'node:test';
import { stripVTControlCharacters } from 'node:util';
import { parseConfig } from '../src/lib/config.mjs';
import { createDispatcher, startRepl } from '../src/repl.mjs';
import { formatTray } from '../src/shell/tray.mjs';

const display = { issue: 108, branch: 'issue-108', seat: 'coder', state: 'drafting',
  model: 'deepseek-v4.1', host: '192.168.1.48:8888', effort: 'l',
  contextUsed: undefined, contextMax: 1048576, startedAt: 0, busy: true };
const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

test('tray uses bright cyan labels and white values without blue or dim text', () => {
  const frame = formatTray(display, { columns: 120, now: 130000 });
  assert.equal(stripVTControlCharacters(frame.top), 'roster | #108 | coder | drafting | issue-108');
  assert.equal(stripVTControlCharacters(frame.bottom),
    'deepseek-v4.1 | 192.168.1.48:8888 | effort low | ctx - / 1.0m | 2m 10s | debug off');
  assert.equal(stripVTControlCharacters(frame.prompt), '* roster> ');
  const fixture = `${frame.top}\n${frame.bottom}\n${frame.prompt}`;
  assert.match(fixture, /\x1b\[96m/);
  assert.match(fixture, /\x1b\[97m/);
  assert.doesNotMatch(fixture, /\x1b\[34/);
  assert.doesNotMatch(fixture, /\x1b\[2m/);
  assert.match(frame.prompt, /\x1b\[96m\*/);
  assert.match(frame.prompt, /\x1b\[96mroster> /);
  assert.match(frame.bottom, /\x1b\[96mctx/);
  assert.match(frame.bottom, /\x1b\[97m- \/ 1\.0m/);
  assert.match(formatTray({ ...display, state: 'failed' }).top, /\x1b\[91mfailed/);
  assert.match(formatTray({ ...display, state: 'passed' }).top, /\x1b\[92mpassed/);
});

test('narrow trays drop elapsed then host and keep every bar within the terminal width', () => {
  const wide = formatTray(display, { columns: 120, color: false, now: 130000 });
  const noElapsed = formatTray(display, { columns: wide.bottom.length, color: false, now: 130000 });
  assert.doesNotMatch(noElapsed.bottom, /2m 10s/);
  assert.match(noElapsed.bottom, /192\.168/);
  const noHost = formatTray(display, { columns: 60, color: false, now: 130000 });
  assert.doesNotMatch(noHost.bottom, /192\.168/);
  assert.match(noHost.bottom, /ctx - \/ 1\.0m/);
  for (const columns of [10, 25, 60, 120]) {
    const frame = formatTray(display, { columns });
    assert.ok(stripVTControlCharacters(frame.top).length < columns);
    assert.ok(stripVTControlCharacters(frame.bottom).length < columns);
  }
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
  await shell.dispatch('/statusbar off');
  assert.equal(shell.state.statusbar, false);
  assert.match(messages, /Status bars off\.\n/);
  assert.doesNotMatch(messages, /\x1b\[/);
  await shell.dispatch('/statusbar on');
  assert.equal(shell.state.statusbar, true);
});

test('TTY shell contains the banner once, top bar, bottom bar and bright cyan input', async () => {
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
  assert.equal(plain.match(/github-agent-roster/g)?.length, 1);
  assert.match(plain, /roster \| local \| coder \| idle \| main/);
  assert.match(plain, /effort medium \| ctx - \/ -/);
  assert.match(plain, /roster> /);
  input.destroy();
  output.destroy();
});
