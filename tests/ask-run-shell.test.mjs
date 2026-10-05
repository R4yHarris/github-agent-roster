import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { PassThrough } from 'node:stream';
import { setTimeout as wait } from 'node:timers/promises';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createDispatcher, startRepl } from '../src/repl.mjs';
import { RunCancelledError } from '../src/runtime/cancel.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

function shell(services) {
  let text = '';
  const result = createDispatcher({ config, env: {}, output: { write(value) { text += value; } },
    errorOutput: { write(value) { text += value; } },
    services: { repositoryBranch: () => 'main', ...services } });
  return { ...result, get text() { return text; } };
}

test('slash and plain asks run locally; retry reuses the exact prepared worktree', async () => {
  const calls = [];
  const prepared = { local: true, task: 'local-0123456789abcdef',
    worktreePath: path.join(process.cwd(), '.worktrees', 'local-0123456789abcdef'), askKind: 'slice' };
  const instance = shell({ submitAsk: () => assert.fail('No GitHub issue creation'),
    runBuiltinAsk: async (ask, options) => {
      calls.push([ask, options.preparedRun]);
      options.onPrepared(prepared);
      return prepared;
    } });
  await instance.dispatch('/ask Add a Status section to README.md.');
  await instance.dispatch('/retry');
  assert.equal(calls.length, 2);
  assert.equal(calls[0][1], undefined);
  assert.equal(calls[1][1], prepared);
  await instance.dispatch('Update docs/guide.md.');
  assert.equal(calls[2][0], 'Update docs/guide.md.');
});

test('confirm pauses after the task summary and Enter continues the same prepared issue without confirm', async () => {
  const calls = [];
  const prepared = { issue: { number: 108 }, task: 'issue-108', askKind: 'slice', confirmedPause: true,
    planningOnly: true, worktreePath: path.join(process.cwd(), '.worktrees', 'issue-108') };
  const instance = shell({ runBuiltinIssue: async (issue, options) => {
    calls.push([issue, options.confirm, options.preparedRun]);
    return options.confirm ? prepared : { ...prepared, confirmedPause: false, planningOnly: false,
      review: { verdict: 'pass' } };
  } });
  await instance.dispatch('/run 108 --confirm');
  assert.equal(instance.state.pendingConfirm.kind, 'run');
  assert.match(instance.text, /Press Enter to continue, or \/stop to cancel/);
  await instance.dispatch('');
  assert.deepEqual(calls, [['108', true, undefined], ['108', false, prepared]]);
  assert.equal(instance.state.pendingConfirm, null);
  assert.equal(instance.state.display.state, 'passed');
});

test('failed planning is not displayed as a confirm pause and cannot continue on Enter', async () => {
  let calls = 0;
  const instance = shell({ runBuiltinIssue: async () => {
    calls += 1;
    return { issue: { number: 176 }, task: 'issue-176', askKind: 'slice',
      planningOnly: true, failed: true, command: null };
  } });
  await instance.dispatch('/run 176');
  assert.equal(instance.state.display.state, 'failed');
  assert.equal(instance.state.pendingConfirm, null);
  assert.match(instance.text, /Planning failed; stubs are unverified\. Coder, tests, reviewer, and publication did not run/);
  assert.doesNotMatch(instance.text, /Paused by --confirm|Press Enter|Use \/publish/);
  await instance.dispatch('');
  assert.equal(calls, 1);
});

test('a failed review shows its reason and the configured bypass instead of offering approved publication', async () => {
  const instance = shell({ runBuiltinIssue: async () => ({
    issue: { number: 176 }, task: 'issue-176', askKind: 'slice',
    review: { verdict: 'fail', reasons: ['Reviewer found no task diff to inspect'] },
    command: 'publish command',
  }) });
  instance.state.config = { ...instance.state.config, review: { required: false }, reviewer: { required: false } };
  await instance.dispatch('/run 176');
  assert.equal(instance.state.display.state, 'failed');
  assert.match(instance.text, /Review failed: Reviewer found no task diff/);
  assert.match(instance.text, /configured review gate is disabled[\s\S]*not approve/);
  assert.doesNotMatch(instance.text, /Use \/publish to publish reviewed changes|Planning failed/);
});

test('stop cancels a confirmed handoff and retry fails explicitly without a prepared worktree', async () => {
  const instance = shell({ runBuiltinIssue: async () => ({ issue: { number: 108 }, task: 'issue-108',
    confirmedPause: true, planningOnly: true, askKind: 'slice', worktreePath: 'issue-108' }) });
  await assert.rejects(instance.dispatch('/retry'), /No Ask or issue run/);
  await instance.dispatch('/run 108 --confirm');
  await instance.dispatch('/stop');
  assert.equal(instance.state.pendingConfirm, null);
  assert.match(instance.text, /Run cancelled/);
  assert.equal(instance.state.display.busy, false);
});

test('/stop is processed while a seat is still waiting, not queued until the run ends', async (t) => {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-stop-shell-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const input = new PassThrough();
  input.isTTY = true;
  input.setRawMode = () => {};
  const output = new PassThrough();
  output.isTTY = true;
  output.columns = 120;
  let text = '';
  let running = false;
  let aborted = false;
  output.on('data', (data) => { text += data.toString(); });
  const done = startRepl({ input, output, errorOutput: output, cwd: root, config, env: {},
    services: { repositoryRoot: () => root, repositoryBranch: () => 'main',
      runBuiltinIssue: async (_issue, { signal }) => {
        running = true;
        return new Promise((_, reject) => signal.addEventListener('abort', () => {
          aborted = true;
          reject(new RunCancelledError());
        }, { once: true }));
      } } });
  for (let index = 0; index < 100 && !text.includes('roster> '); index += 1) await wait(10);
  input.write('/run 108\n');
  for (let index = 0; index < 100 && !running; index += 1) await wait(10);
  assert.equal(running, true);
  input.write('/stop\n');
  for (let index = 0; index < 100 && !text.includes('Run cancelled.'); index += 1) await wait(10);
  assert.equal(aborted, true);
  input.write('/quit\n');
  assert.equal(await done, 0);
  input.destroy();
  output.destroy();
});
