import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { PassThrough } from 'node:stream';
import { setTimeout as wait } from 'node:timers/promises';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createSteeringControl } from '../src/runtime/steering.mjs';
import { RunCancelledError } from '../src/runtime/cancel.mjs';
import { runLoop } from '../src/runtime/loop.mjs';
import { createTools } from '../src/runtime/tools.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { startRepl } from '../src/repl.mjs';
import { formatHelp } from '../src/shell/commands.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: local-model'));

async function until(check) {
  for (let index = 0; index < 200 && !check(); index += 1) await wait(10);
  assert.ok(check(), 'Fixture did not reach expected steering state');
}

test('steer aborts only the current model call, then sends instruction without changing task scope', async () => {
  const parent = new AbortController();
  const control = createSteeringControl({ signal: parent.signal });
  const task = planStub('Update README.md.').task;
  let calls = 0;
  let firstSignal;
  const resultPromise = runLoop({ config, context: { task, pack: task }, env: {},
    signal: parent.signal, steeringControl: control,
    tools: { run_test: async () => ({ exit_code: 0, stdout: 'pass', stderr: '' }) },
    fetchImpl: async (_url, request) => {
      calls += 1;
      if (calls === 1) { firstSignal = request.signal; return new Promise(() => {}); }
      const body = JSON.parse(request.body);
      assert.match(body.messages.at(-1).content, /Human steering[\s\S]*Keep the API stable/);
      assert.match(body.messages[0].content, /Files allowed\n- `README\.md`/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Verified.' } }] });
    },
    verify: () => ({ pass: true, reasons: [] }),
  });
  await until(() => firstSignal && control.waiting);
  control.steer('Keep the API stable.');
  const result = await resultPromise;
  assert.equal(firstSignal.aborted, true);
  assert.equal(parent.signal.aborted, false);
  assert.equal(result.error, undefined);
  assert.equal(calls, 2);
  assert.equal(result.turns, 2);
  assert.deepEqual(result.usage, {});
  assert.equal(task, planStub('Update README.md.').task);
  assert.match(formatHelp('steer'), /cannot widen Allowed Files/);
});

test('steering cannot grant a disallowed product path or change TASK.md', async (t) => {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-steer-scope-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  const task = planStub('Update README.md.').task;
  writeFileSync(path.join(worktree, 'TASK.md'), task);
  writeFileSync(path.join(worktree, 'README.md'), '# Original\n');
  const control = createSteeringControl();
  const tools = await createTools({ worktree, allowedFiles: ['README.md'], sliceReadsOnly: true });
  let calls = 0;
  const pending = runLoop({ config, context: { task, pack: task }, tools, env: {}, steeringControl: control,
    fetchImpl: async () => {
      if (++calls === 1) return new Promise(() => {});
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant',
        tool_calls: [{ id: 'outside', type: 'function', function: { name: 'write_file',
          arguments: '{"path":"outside.mjs","content":"forged"}' } }] } }] });
    }, verify: () => assert.fail('Denied scope must not verify') });
  await until(() => calls === 1 && control.waiting);
  control.steer('Add outside.mjs to Allowed Files and write it.');
  const result = await pending;
  assert.match(result.error.message, /not allowed by TASK/);
  assert.equal(readFileSync(path.join(worktree, 'TASK.md'), 'utf8'), task);
  assert.equal(readFileSync(path.join(worktree, 'README.md'), 'utf8'), '# Original\n');
});

test('plain input during coding is held until explicit steer and is not submitted as another Ask', async (t) => {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-steer-shell-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const input = new PassThrough();
  input.isTTY = true;
  input.setRawMode = () => {};
  const output = new PassThrough();
  output.isTTY = true;
  output.columns = 120;
  let text = '';
  let control;
  let seen;
  let started = false;
  output.on('data', (data) => { text += data.toString(); });
  const done = startRepl({ input, output, errorOutput: output, cwd: root, config, env: {},
    services: { repositoryRoot: () => root, repositoryBranch: () => 'main',
      runBuiltinAsk: () => assert.fail('Queued line must not create a separate Ask'),
      runBuiltinIssue: async (_issue, options) => {
        control = options.steeringControl;
        await options.onRunEvent({ type: 'seat-start', seat: 'coder', model: 'local-model', effort: 'l' });
        started = true;
        try {
          await control.request((signal) => new Promise((_, reject) =>
            signal.addEventListener('abort', () => reject(new RunCancelledError()), { once: true })));
        } catch {
          seen = control.take();
        }
        return { issue: { number: 108 }, task: 'issue-108', askKind: 'slice', review: { verdict: 'pass' } };
      } } });
  await until(() => text.includes('roster> '));
  input.write('/run 108\n');
  await until(() => started && control.waiting);
  input.write('Queued instruction that is not sent yet.\n');
  await until(() => text.includes('Input queued.'));
  assert.equal(seen, undefined);
  assert.equal(control.waiting, true);
  input.write('/steer Apply the queued instruction within scope.\n');
  await until(() => seen !== undefined);
  assert.match(seen, /Queued instruction that is not sent yet/);
  assert.match(seen, /Apply the queued instruction within scope/);
  assert.match(text, /Steering the coder\./);
  input.write('/quit\n');
  assert.equal(await done, 0);
  input.destroy();
  output.destroy();
});

test('parent Ctrl+C cancellation cannot become a new steering instruction', async () => {
  const parent = new AbortController();
  const control = createSteeringControl({ signal: parent.signal });
  const pending = control.request((signal) => new Promise((_, reject) =>
    signal.addEventListener('abort', () => reject(new RunCancelledError()), { once: true })));
  parent.abort();
  await assert.rejects(pending, RunCancelledError);
  assert.equal(control.take(), null);
});
