import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { PassThrough } from 'node:stream';
import { setTimeout as wait } from 'node:timers/promises';
import test from 'node:test';
import { stripVTControlCharacters } from 'node:util';
import { parseConfig } from '../src/lib/config.mjs';
import { createBuiltinChat } from '../src/lib/llm.mjs';
import { startRepl } from '../src/repl.mjs';
import { RunCancelledError } from '../src/runtime/cancel.mjs';
import { completeCommand } from '../src/shell/commands.mjs';
import { createHistory, safeHistoryLine } from '../src/shell/history.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

function fixture(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-shell-keys-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

async function until(check) {
  for (let count = 0; count < 200; count += 1) {
    if (check()) return;
    await wait(10);
  }
  throw new Error('Shell fixture did not reach the expected state');
}

function tty(t, root, services = {}) {
  const input = new PassThrough();
  input.isTTY = true;
  input.setRawMode = () => {};
  const output = new PassThrough();
  output.isTTY = true;
  output.columns = 120;
  let text = '';
  output.on('data', (data) => { text += data.toString(); });
  const done = startRepl({ input, output, errorOutput: output, cwd: root, config, env: {},
    services: { repositoryRoot: () => root, repositoryBranch: () => 'main', ...services } });
  t.after(() => { input.destroy(); output.destroy(); });
  return { input, done, get text() { return stripVTControlCharacters(text); }, get raw() { return text; } };
}

test('history keeps the last 200 safe owner-only lines and never records vault, PEM, token or password input', async (t) => {
  const root = fixture(t);
  const history = createHistory({ repoRoot: root, env: { ROSTER_API_KEY: 'test-only-secret-value' } });
  await history.load();
  for (let index = 0; index < 205; index += 1) await history.record(`/run ${index + 1}`);
  for (const line of ['/vault set ROSTER_TOKEN', 'C:\\private\\key.pem', '/ask password=private',
    '/ask token ghp_abcdefghijkabcdefghijk', 'test-only-secret-value']) await history.record(line);
  await history.record('private-vault-value', { secret: true });
  assert.equal(history.lines.length, 200);
  assert.equal(history.lines[0], '/run 6');
  assert.equal(history.lines.at(-1), '/run 205');
  const saved = readFileSync(history.path, 'utf8');
  assert.doesNotMatch(saved, /private|vault|token|secret|\.pem/i);
  if (process.platform !== 'win32') assert.equal(statSync(history.path).mode & 0o777, 0o600);
  const reopened = createHistory({ repoRoot: root, env: {} });
  assert.deepEqual(await reopened.load(), history.lines);
  assert.equal(safeHistoryLine('password: private'), false);
});

test('Up recalls the last command but does not execute it before Enter', async (t) => {
  const root = fixture(t);
  mkdirSync(path.join(root, '.roster'));
  writeFileSync(path.join(root, '.roster', 'history'), '/run 42\n');
  const calls = [];
  const shell = tty(t, root, { runBuiltinIssue: async (issue) => {
    calls.push(issue);
    return { issue: { number: 42 }, task: 'issue-42' };
  } });
  await until(() => shell.text.includes('roster> '));
  shell.input.write('\x1b[A');
  await until(() => shell.text.includes('/run 42'));
  assert.deepEqual(calls, []);
  shell.input.write('\n');
  await until(() => calls.length === 1);
  shell.input.write('/quit\n');
  assert.equal(await shell.done, 0);
  assert.deepEqual(calls, ['42']);
});

test('pasted vault values are absent from disk and readline history', async (t) => {
  const root = fixture(t);
  let stored;
  const shell = tty(t, root, { createFileVault: () => ({ set: async (_name, value) => { stored = value; } }) });
  await until(() => shell.text.includes('roster> '));
  shell.input.write('/vault set SECRET\nprivate-vault-value\n/quit\n');
  assert.equal(await shell.done, 0);
  assert.equal(stored, 'private-vault-value');
  const file = path.join(root, '.roster', 'history');
  if (existsSync(file)) assert.doesNotMatch(readFileSync(file, 'utf8'), /vault|private-vault-value/);
  assert.doesNotMatch(shell.text, /private-vault-value/);
});

test('Ctrl+C cancels an in-flight seat and returns to the prompt; Ctrl+D exits zero', async (t) => {
  const root = fixture(t);
  let started = false;
  let aborted = false;
  const shell = tty(t, root, { runBuiltinIssue: async (_issue, { signal }) => {
    started = true;
    return new Promise((_, reject) => signal.addEventListener('abort', () => {
      aborted = true;
      reject(new RunCancelledError());
    }, { once: true }));
  } });
  await until(() => shell.text.includes('roster> '));
  shell.input.write('/run 42\n');
  await until(() => started);
  shell.input.write('\x03');
  await until(() => aborted && shell.text.includes('Run cancelled.'));
  assert.match(shell.text, /idle/);
  shell.input.write('\x04');
  assert.equal(await shell.done, 0);
});

test('second Ctrl+C, aliases and exit leave with zero', async (t) => {
  for (const command of ['/q\n', '/quit\n', 'exit\n', '\x04']) {
    const root = fixture(t);
    const shell = tty(t, root);
    await until(() => shell.text.includes('roster> '));
    shell.input.write(command);
    assert.equal(await shell.done, 0);
  }
  const root = fixture(t);
  let started = false;
  const shell = tty(t, root, { runBuiltinIssue: async (_issue, { signal }) => {
    started = true;
    return new Promise((_, reject) => signal.addEventListener('abort', () => reject(new RunCancelledError())));
  } });
  await until(() => shell.text.includes('roster> '));
  shell.input.write('/run 42\n');
  await until(() => started);
  shell.input.write('\x03\x03');
  assert.equal(await shell.done, 0);
});

test('Tab completes registry commands and redraw does not clear scrollback', async (t) => {
  assert.deepEqual(completeCommand('/sta'), [['/stats', '/status', '/statusbar'], '/sta']);
  assert.deepEqual(completeCommand('/qui'), [['/quit'], '/qui']);
  assert.deepEqual(completeCommand('/run 42'), [[], '/run 42']);
  const root = fixture(t);
  const shell = tty(t, root);
  await until(() => shell.text.includes('roster> '));
  shell.input.write('/sta');
  await wait(10);
  shell.input.write('\t');
  await wait(10);
  shell.input.write('\t');
  await until(() => shell.text.includes('/statusbar'));
  shell.input.write('\x15');
  shell.input.write('/redraw\n');
  await wait(50);
  assert.ok(!shell.raw.includes('\x1b[2J'));
  shell.input.write('/clear\n');
  await until(() => shell.raw.includes('\x1b[2J\x1b[H'));
  shell.input.write('/quit\n');
  assert.equal(await shell.done, 0);
});

test('/help lists the core commands and /statusbar off removes the pinned rules', async (t) => {
  const root = fixture(t);
  const shell = tty(t, root);
  await until(() => shell.text.includes('roster> '));
  shell.input.write('/help\n');
  await until(() => shell.text.includes('/statusbar on|off'));
  for (const usage of ['/run ', '/publish ', '/debug ', '/usage', '/statusbar ', '/quit']) {
    assert.ok(shell.text.includes(usage), usage);
  }
  shell.input.write('/statusbar off\n');
  await until(() => shell.text.includes('Status bars off.'));
  await wait(20);
  assert.doesNotMatch(shell.text.slice(shell.text.lastIndexOf('Status bars off.')), /\u2500{10}/);
  assert.doesNotMatch(shell.raw, /\x1b\[34/);
  assert.doesNotMatch(shell.raw, /\x1b\[2m/);
  shell.input.write('/quit\n');
  assert.equal(await shell.done, 0);
});

test('cancellation aborts the actual model request instead of leaving it waiting for the timeout', async () => {  const controller = new AbortController();
  let requestSignal;
  const chat = createBuiltinChat({ ...config, llm: { ...config.llm,
    base_url: 'http://localhost:8000/v1', model: 'test-model' } }, {
    env: {}, signal: controller.signal, fetchImpl: async (_url, request) => {
      requestSignal = request.signal;
      return new Promise(() => {});
    },
  });
  const response = chat({ messages: [{ role: 'user', content: 'Task' }] });
  await until(() => requestSignal);
  controller.abort();
  await assert.rejects(response, RunCancelledError);
  assert.equal(requestSignal.aborted, true);
});
