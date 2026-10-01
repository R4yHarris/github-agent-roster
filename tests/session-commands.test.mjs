import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { createDebugLog } from '../src/lib/debug-log.mjs';
import { createRunLog, readLastRunLog } from '../src/lib/run-log.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

function shell(options = {}) {
  let text = '';
  const dispatcher = createDispatcher({ config, env: {}, ...options,
    output: { write(value) { text += value; } }, errorOutput: { write(value) { text += value; } },
    services: { repositoryBranch: () => 'main',
      readStatus: () => assert.fail('Cached status must not call GitHub'),
      runBuiltinAsk: () => assert.fail('Status must not call a model'), ...options.services } });
  return { ...dispatcher, get text() { return text; } };
}

test('status has every session field without any network even before the first run', async () => {
  const instance = shell();
  await instance.dispatch('/status');
  for (const field of ['Issue', 'Branch', 'Seat', 'State', 'Model', 'Host', 'Effort',
    'Last finish reason', 'Last test name', 'Review']) assert.ok(instance.text.includes(`${field}:`));
  assert.match(instance.text, /Issue: local/);
  assert.match(instance.text, /Last finish reason: -/);
});

test('run events populate finish, tests and review; cached previous issues remain offline', async () => {
  const instance = shell({ services: { runBuiltinIssue: async (number, options) => {
    await options.onRunEvent({ type: 'seat-start', seat: 'coder', model: 'served-model', host: 'localhost',
      effort: 'l', contextMax: 1048576 });
    await options.onRunEvent({ type: 'completion', seat: 'coder', reason: 'stop' });
    await options.onRunEvent({ type: 'tool', seat: 'coder', name: 'run_test' });
    return { issue: { number: Number(number), title: 'Cached issue' }, task: `issue-${number}`,
      worktreePath: path.join(process.cwd(), `.worktrees`, `issue-${number}`), review: { verdict: 'pass' } };
  } } });
  await instance.dispatch('/run 108');
  await instance.dispatch('/run 109');
  await instance.dispatch('/status 108');
  assert.match(instance.text, /Issue: #108[\s\S]*Branch: issue-108[\s\S]*Model: served-model/);
  assert.match(instance.text, /Last finish reason: stop\nLast test name: node --test\nReview: pass/);
});

test('history shows only the last 20 safe entries and debug status is process-only', async () => {
  const instance = shell({ services: { setConfigValue: () => assert.fail('Session controls must not write config') } });
  instance.state.history = [...Array.from({ length: 25 }, (_, index) => `/run ${index + 1}`),
    '/vault set SECRET', 'secret.pem', 'password=private'];
  await instance.dispatch('/history');
  assert.match(instance.text, /^\/run 6\n/);
  assert.match(instance.text, /\/run 25\n/);
  assert.doesNotMatch(instance.text, /vault|password|secret\.pem/);
  await instance.dispatch('/debug status');
  assert.match(instance.text, /Debug logging off/);
  await instance.dispatch('/statusbar off');
  assert.equal(instance.state.statusbar, false);
});

test('debug off creates no file, and a length event contains only metadata when enabled', async (t) => {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-session-debug-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const debug = createDebugLog({ env: {}, session: 'session-command-test' });
  const instance = shell({ debug });
  const context = { repoRoot: root, issue: 108, seat: 'coder',
    event: { type: 'finish-reason', reason: 'length', retry: true, content: 'PRIVATE_BODY' } };
  await debug.record(context);
  assert.equal(existsSync(path.join(root, '.roster', 'logs')), false);
  await instance.dispatch('/debug on');
  await debug.record(context);
  const row = JSON.parse(readFileSync(debug.path, 'utf8').trim());
  assert.equal(row.finish_reason, 'length');
  assert.equal(row.path_class, null);
  assert.doesNotMatch(readFileSync(debug.path, 'utf8'), /PRIVATE_BODY/);
  const before = readFileSync(debug.path, 'utf8');
  await instance.dispatch('/debug off');
  await debug.record(context);
  assert.equal(readFileSync(debug.path, 'utf8'), before);
});

test('supported completion reasons remain safe and visible in bounded seat log tails', async (t) => {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-session-tail-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const logger = await createRunLog({ repoRoot: root, session: 'roster-108-coder', env: {},
    errorOutput: { write() {} } });
  await logger.seat('coder', 'roster-108-coder', config, async (onEvent) => {
    await onEvent({ type: 'completion', reason: 'stop', content: 'PRIVATE_COMPLETION' });
    return {};
  });
  const tail = await readLastRunLog({ repoRoot: root, session: logger.session, limit: 50, env: {} });
  assert.ok(tail.lines.some((line) => line.includes('completion finish_reason="stop"')));
  assert.doesNotMatch(tail.lines.join('\n'), /PRIVATE_COMPLETION/);
});
