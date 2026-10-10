import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { PassThrough } from 'node:stream';
import { setTimeout as wait } from 'node:timers/promises';
import { stripVTControlCharacters } from 'node:util';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createDispatcher, startRepl } from '../src/repl.mjs';
import { RunCancelledError } from '../src/runtime/cancel.mjs';
import { classifyAsk, clarificationHint } from '../src/planner/classify.mjs';
import { runBuiltinAsk } from '../src/lib/builtin.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

function shell(services) {
  let text = '';
  const result = createDispatcher({ config, env: {}, output: { write(value) { text += value; } },
    errorOutput: { write(value) { text += value; } },
    services: { repositoryBranch: () => 'main', ...services } });
  return { ...result, get text() { return text; } };
}

function clarificationFixture(t) {
  const root = mkdtempSync(path.join(process.cwd(), '.clarification-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const readme = '# Unchanged product\n';
  const events = [];
  const prepared = [];
  const env = { PATH: process.env.PATH, SystemRoot: process.env.SystemRoot,
    ROSTER_PROVENANCE_OPT_OUT: 'true' };
  const localConfig = { ...config, start: { base: 'current', sync: 'offline' },
    llm: { ...config.llm, base_url: 'http://127.0.0.1:1/v1', model: 'test-model' } };
  const options = { cwd: root, config: localConfig, env,
    // Only Git preparation is simulated; classification, logging and seat boundaries are real.
    runCommand: async (program, args) => {
      assert.equal(program, 'git');
      if (args.join(' ') === 'rev-parse --show-toplevel') return root;
      if (args.join(' ') === 'remote get-url origin') return '';
      assert.deepEqual(args.slice(0, 3), ['worktree', 'add', '-b']);
      const worktree = args[4];
      mkdirSync(worktree, { recursive: true });
      execFileSync('git', ['init', '--quiet', '-b', args[3]], { cwd: worktree, env });
      writeFileSync(path.join(worktree, 'README.md'), readme);
      return '';
    },
    fetchImpl: () => assert.fail('Clarification must not call any model'),
    runTestCommand: () => assert.fail('Clarification must not run implementation tests'),
    publisher: () => assert.fail('Clarification must not publish'),
    onPrepared: (run) => prepared.push(run),
    onRunEvent: (event) => events.push(event),
  };
  function assertNoSeats(result) {
    assert.equal(result.askKind, 'clarify');
    assert.equal(result.planningOnly, true);
    assert.equal(result.failed, false);
    assert.equal(result.command, null);
    assert.deepEqual(result.runs, { planner: null, coder: null, reviewer: null });
    assert.equal(result.run, null);
    assert.deepEqual(events, []);
    for (const run of prepared) {
      assert.equal(readFileSync(path.join(run.worktreePath, 'README.md'), 'utf8'), readme);
      assert.deepEqual(readdirSync(run.worktreePath).sort(), ['.git', 'ASSIGNMENT.md', 'README.md']);
    }
    assert.equal(existsSync(path.join(root, '.roster', 'runs')), false);
  }
  return { root, options, assertNoSeats };
}

test('an actually ambiguous local ask prints one clarification notice and keeps the public shell usable', async (t) => {
  const ambiguousAsk = 'Improve the product overall.';
  assert.equal(classifyAsk(ambiguousAsk).kind, 'clarify');
  const reason = classifyAsk(ambiguousAsk).reason;
  const fixture = clarificationFixture(t);
  const input = new PassThrough();
  input.isTTY = true;
  input.setRawMode = () => {};
  const output = new PassThrough();
  output.isTTY = true;
  output.columns = 120;
  let text = '';
  let result;
  let normalLocal = 0;
  output.on('data', (data) => { text += data.toString(); });
  const done = startRepl({ input, output, errorOutput: output,
    cwd: fixture.root, config: fixture.options.config, env: fixture.options.env,
    historyStore: { lines: [], load: async () => [], record: async () => {}, flush: async () => {} },
    services: { repositoryBranch: () => 'main', probeModelDetails: async () => [],
      runBuiltinAsk: async (ask, options) => {
      if (ask === ambiguousAsk) {
        result = await runBuiltinAsk(ask, { ...options, ...fixture.options,
          onPrepared: (prepared) => {
            fixture.options.onPrepared(prepared);
            options.onPrepared(prepared);
          },
          onRunEvent: (event) => {
            fixture.options.onRunEvent(event);
            options.onRunEvent(event);
          },
        });
        return result;
      }
      assert.equal(ask, 'Add a Status section to README.md.');
      normalLocal += 1;
      options.log(`Ask kind: slice (bounded one-file slice)`);
      options.log('Task summary: ready');
      return { local: true, task: 'local-fedcba9876543210', askKind: 'slice',
        planningOnly: false, failed: false, command: null,
        runs: { planner: null, coder: null, reviewer: null }, run: null };
      },
      runBuiltinIssue: () => assert.fail('An ambiguous local ask must not become an issue run'),
    },
  });
  t.after(async () => {
    input.write('/quit\n');
    await done;
    input.destroy();
    output.destroy();
  });
  async function until(check) {
    for (let index = 0; index < 200 && !check(); index += 1) await wait(10);
    assert.ok(check(), text);
  }
  const lastRail = () => stripVTControlCharacters(text).split(/\r?\n/)
    .filter((line) => line.includes(' │ ')).at(-1) ?? '';
  await until(() => text.includes('roster> '));
  input.write(`${ambiguousAsk}\n`);
  await until(() => result && lastRail().includes('idle │'));
  assert.equal(text.split(clarificationHint).length - 1, 1);
  assert.ok(text.includes(`Ask kind: clarify (${reason})`));
  fixture.assertNoSeats(result);
  assert.match(lastRail(), /idle │/);
  assert.doesNotMatch(text, /Live log|RECIPE:|RESULT:|REVIEW:|Use \/publish|Success|finished|Error:/);
  input.write('/help\n');
  await until(() => text.includes('Commands:'));
  input.write('Add a Status section to README.md.\n');
  await until(() => normalLocal === 1 && text.includes('Use /publish to publish reviewed changes'));
  assert.equal(normalLocal, 1);
  assert.match(lastRail(), /idle │/);
  assert.match(text, /Use \/publish to publish reviewed changes/);
  input.write('/quit\n');
  assert.equal(await done, 0);
});

test('a direct ambiguous builtin ask keeps shared logging, reason and no-seat result', async (t) => {
  const fixture = clarificationFixture(t);
  const ask = 'Improve the product overall.';
  const notices = [];
  const result = await runBuiltinAsk(ask, { ...fixture.options, log: (message) => notices.push(message) });
  assert.ok(notices.includes(`Ask kind: clarify (${classifyAsk(ask).reason})`));
  assert.equal(notices.filter((message) => message === clarificationHint).length, 1);
  assert.equal(result.classification.reason, classifyAsk(ask).reason);
  assert.equal(result.clarification, clarificationHint);
  fixture.assertNoSeats(result);
});

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
