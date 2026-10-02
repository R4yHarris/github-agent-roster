import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { PassThrough } from 'node:stream';
import { setTimeout as wait } from 'node:timers/promises';
import { parseConfig } from '../src/lib/config.mjs';
import { askSideQuestion } from '../src/lib/side-question.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { createDispatcher, startRepl } from '../src/repl.mjs';
import { RunCancelledError } from '../src/runtime/cancel.mjs';
import { formatHelp } from '../src/shell/commands.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: local-model'));

function fixture(t) {
  const worktreePath = mkdtempSync(path.join(tmpdir(), 'roster-btw-'));
  t.after(() => rmSync(worktreePath, { recursive: true, force: true }));
  writeFileSync(path.join(worktreePath, 'TASK.md'), planStub('Update README.md.').task);
  writeFileSync(path.join(worktreePath, 'README.md'), '# Product\n');
  return { worktreePath, task: 'issue-108', result: { summary: 'Original publish summary.' } };
}

test('side question makes one tool-free request and changes no task or product file', async (t) => {
  const run = fixture(t);
  const before = readFileSync(path.join(run.worktreePath, 'TASK.md'), 'utf8');
  let requests = 0;
  const answer = await askSideQuestion({ question: 'Which check covers the change?', run, config, env: {},
    fetchImpl: async (_url, request) => {
      requests += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.tools, undefined);
      assert.equal(body.tool_choice, undefined);
      assert.match(body.messages[0].content, /read-only/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'The acceptance check.' } }] });
    } });
  assert.equal(requests, 1);
  assert.equal(answer, 'The acceptance check.');
  assert.equal(readFileSync(path.join(run.worktreePath, 'TASK.md'), 'utf8'), before);
  assert.equal(readFileSync(path.join(run.worktreePath, 'README.md'), 'utf8'), '# Product\n');
});

test('tool requests and length are refused without executing writes/tests or adding a second request', async (t) => {
  const run = fixture(t);
  let requests = 0;
  await assert.rejects(askSideQuestion({ question: 'Edit the file?', run, config, env: {},
    fetchImpl: async () => {
      requests += 1;
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', content: null,
        tool_calls: [{ id: 'write', type: 'function', function: { name: 'write_file',
          arguments: '{"path":"README.md","content":"forged"}' } }] } }] });
    } }), /read-only/);
  assert.equal(requests, 1);
  requests = 0;
  await assert.rejects(askSideQuestion({ question: 'Explain?', run, config, env: {},
    fetchImpl: async () => {
      requests += 1;
      return Response.json({ choices: [{ finish_reason: 'length', message: { role: 'assistant', content: 'truncated' } }] });
    } }), /finish reason: length/);
  assert.equal(requests, 1);
  assert.equal(readFileSync(path.join(run.worktreePath, 'README.md'), 'utf8'), '# Product\n');
});

test('shell prints the answer without altering seat state, memory or publication summary', async (t) => {
  const run = fixture(t);
  let text = '';
  const shell = createDispatcher({ config, env: {}, output: { write(value) { text += value; } },
    errorOutput: { write() {} }, services: { repositoryBranch: () => 'issue-108',
      askSideQuestion: async () => 'Side answer only.' } });
  shell.state.lastRun = run;
  const display = { ...shell.state.display };
  const original = JSON.stringify(run);
  await shell.dispatch('/btw Explain the current check.');
  assert.equal(text, 'Side answer only.\n');
  assert.deepEqual(shell.state.display, display);
  assert.equal(JSON.stringify(shell.state.lastRun), original);
  assert.match(formatHelp('btw'), /read-only/);
});

test('a side question answers during a running seat without cancelling that seat', async (t) => {
  const run = fixture(t);
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
  const done = startRepl({ input, output, errorOutput: output, cwd: run.worktreePath, config, env: {},
    services: { repositoryRoot: () => run.worktreePath, repositoryBranch: () => 'main',
      runBuiltinIssue: async (_issue, options) => {
        options.onPrepared(run);
        running = true;
        return new Promise((_, reject) => options.signal.addEventListener('abort', () => {
          aborted = true;
          reject(new RunCancelledError());
        }, { once: true }));
      },
      askSideQuestion: async () => 'Read-only side answer.',
    } });
  const until = async (check) => {
    for (let index = 0; index < 200 && !check(); index += 1) await wait(10);
    assert.ok(check());
  };
  await until(() => text.includes('roster> '));
  input.write('/run 108\n');
  await until(() => running);
  input.write('/btw What does the check cover?\n');
  await until(() => text.includes('Read-only side answer.'));
  assert.equal(aborted, false);
  input.write('/quit\n');
  assert.equal(await done, 0);
  input.destroy();
  output.destroy();
});
