import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync, mkdtempSync, rmSync, existsSync, mkdirSync, writeFileSync, symlinkSync, cpSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
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
import { buildPlan } from '../src/planner/stub.mjs';
import { parseTaskDocument } from '../src/planner/task.mjs';
import { readPlannerHandoff } from '../src/seats/planner.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';
import { passingReview } from './helpers/review.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

const directoryAsk = 'Write me a one page design of a simple marketing web page for a Security and Risk Analaysis consulting business and setup instructions for obtaining a web page and serving the web page to market the business. Output it in a new "new-design" directory';
const directoryPlan = { title: 'One-page design and setup instructions',
  files_allowed: ['new-design/DESIGN.md', 'new-design/SETUP.md'],
  acceptance_checks: ['DESIGN describes the one-page consulting business marketing web page',
    'SETUP documents obtaining and serving the page without purchases or deployment'],
  task_class: 'docs', difficulty: 1, estimate_min: 8 };
const localConfig = { ...config, planner: { ...config.planner, turn_budget: 2 },
  seat: { ...config.seat, turn_budget: 8 }, start: { base: 'current', sync: 'offline' },
  llm: { ...config.llm, base_url: 'http://localhost:1234/v1', model: 'fake-directory-model' } };

function directoryFixture(t) {
  const root = path.join(process.cwd(), 'tests', `.directory-ask-${randomUUID()}`);
  mkdirSync(root);
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
  writeFileSync(path.join(root, '.gitignore'), '.worktrees/\n.roster/\n');
  writeFileSync(path.join(root, 'README.md'), '# Main checkout stays unchanged\n');
  git('init', '-b', 'main');
  git('add', '--all');
  git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'Fixture');
  mkdirSync(path.join(root, '.roster', 'machine'), { recursive: true });
  const repoRoot = path.join(root, '.roster', 'installation');
  mkdirSync(repoRoot);
  for (const name of ['principals', 'skills']) {
    cpSync(new URL(`../${name}/`, import.meta.url), path.join(repoRoot, name), { recursive: true });
  }
  writeFileSync(path.join(repoRoot, 'roster.config.example.yml'),
    readFileSync(new URL('../roster.config.example.yml', import.meta.url)));
  const env = { ...process.env, ROSTER_SEAT: undefined, GITHUB_APP_ID: undefined,
    GITHUB_APP_PRIVATE_KEY_PATH: undefined, AI_MODEL: '', ROSTER_MODEL: '',
    GITHUB_AGENT_CONTRACTS: resolveContractsPath(),
    ROSTER_STATE_ROOT: path.join(root, '.roster', 'machine') };
  return { root, repoRoot, git, env };
}

test('the exact human prompt executes the public local shell planner→coder→reviewer and writes isolated docs', async (t) => {
  const { root, repoRoot, git, env } = directoryFixture(t);
  const before = readFileSync(path.join(root, 'README.md'));
  const head = git('rev-parse', 'HEAD');
  const seats = [];
  const files = {
    'new-design/DESIGN.md': '# One-page design\n\nSecurity and Risk Analysis consulting: hero, services, trust and contact.\n',
    'new-design/SETUP.md': '# Setup instructions\n\nCompare domains and static hosting; obtain and serve the page only after human approval.\n',
  };
  let coderTurns = 0;
  const fetchImpl = async (_url, request) => {
    const body = JSON.parse(request.body);
    const system = body.messages[0].content;
    const reply = (content) => Response.json({ model: 'fake-directory-model',
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content } }] });
    if (system.includes('builtin planner seat')) {
      seats.push('planner');
      assert.match(system, /only the output directory "new-design"/);
      assert.ok(body.messages[1].content.startsWith(directoryAsk));
      return reply(JSON.stringify(directoryPlan));
    }
    if (system.startsWith('You are the builtin reviewer seat.')) {
      seats.push('reviewer');
      return reply(passingReview(body));
    }
    assert.doesNotMatch(system, /critic|research/i);
    if (coderTurns++ === 0) {
      seats.push('coder');
      return Response.json({ model: 'fake-directory-model', choices: [{ finish_reason: 'tool_calls',
        message: { role: 'assistant', tool_calls: Object.entries(files).map(([file, content], index) => ({
          id: `write-${index}`, type: 'function', function: { name: 'write_file',
            arguments: JSON.stringify({ path: file, content }) },
        })) } }] });
    }
    return reply('Check 1 done: new-design/DESIGN.md contains the one-page design. ' +
      'Check 2 done: new-design/SETUP.md contains setup instructions. Docs-only; tests skipped.');
  };
  const instance = createDispatcher({ cwd: root, repoRoot, config: localConfig, env,
    output: { write() {} }, errorOutput: { write() {} },
    services: {
      repositoryBranch: () => 'main',
      submitAsk: () => assert.fail('Local shell must not create an issue'),
      runBuiltinAsk: (ask, options) => runBuiltinAsk(ask, { ...options, autoModel: false, fetchImpl,
        publisher: () => assert.fail('Local docs must not publish'),
        runTestCommand: () => assert.fail('Docs must not execute tests or installation commands') }),
    } });
  await instance.dispatch(directoryAsk);
  const result = instance.state.lastRun;
  assert.deepEqual(seats, ['planner', 'coder', 'reviewer']);
  assert.equal(result.local, true);
  assert.equal(result.failed, false);
  assert.equal(result.review.verdict, 'pass');
  assert.equal(result.result.testsSkipped, true);
  assert.equal(parseTaskDocument(readFileSync(result.taskPath, 'utf8')).ask, directoryAsk);
  assert.ok(existsSync(path.join(result.worktreePath, 'ESTIMATE.md')));
  assert.ok(existsSync(path.join(result.worktreePath, 'RECIPE.yml')));
  for (const [file, content] of Object.entries(files)) {
    assert.equal(readFileSync(path.join(result.worktreePath, file), 'utf8'), content);
    assert.equal(existsSync(path.join(root, file)), false);
  }
  assert.deepEqual(readFileSync(path.join(root, 'README.md')), before);
  assert.equal(git('rev-parse', 'HEAD'), head);
  assert.equal(git('status', '--porcelain'), '');
  assert.match(readFileSync(result.review.reviewPath, 'utf8'), /pass/i);
});

test('public directory runs reject invalid planner files or checks without invoking coder', async (t) => {
  const { root, repoRoot, env } = directoryFixture(t);
  for (const invalid of [
    { files_allowed: ['outside/DESIGN.md'] }, { files_allowed: ['new-design/**'] },
    { files_allowed: ['new-design/.env'] }, { files_allowed: ['new-design/../DESIGN.md'] },
    { files_allowed: ['new-design/vendor/file.md'] }, { files_allowed: ['new-design/agent-policy.yml'] },
    { acceptance_checks: [] }, { acceptance_checks: ['node --test exits 0'] },
    { files_allowed: 'new-design/DESIGN.md' },
    { files_allowed: ['new-design/deploy.sh'], acceptance_checks: ['npm run deploy succeeds'] },
    { files_allowed: ['new-design/index.html'], acceptance_checks: directoryPlan.acceptance_checks },
    { acceptance_checks: ['SETUP documents instructions and npm run deploy succeeds'] },
    { acceptance_checks: ['SETUP describes hosting and the account is created'] },
    { acceptance_checks: ['SETUP describes network access and curl https://example.invalid succeeds'] },
  ]) {
    let calls = 0;
    await assert.rejects(runBuiltinAsk(directoryAsk, { cwd: root, repoRoot, config: localConfig, env, log: () => {},
      errorOutput: { write() {} },
      runTestCommand: () => assert.fail('Invalid handoffs must not execute tests'),
      fetchImpl: async (_url, request) => {
        calls += 1;
        assert.match(JSON.parse(request.body).messages[0].content, /builtin planner seat/);
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant',
          content: JSON.stringify({ ...directoryPlan, ...invalid }),
        } }] });
      } }), /Planner turn budget/);
    assert.equal(calls, 2);
    assert.equal(existsSync(path.join(root, 'new-design')), false);
  }
});

test('public documentation intent denies unsafe plans across verbs and noun-only requests', async (t) => {
  const { root, repoRoot, env } = directoryFixture(t);
  for (const ask of [
    'Generate documentation for the site. Output it in a new `new-design` directory',
    'Compose setup instructions for the site. Output it in a new "new-design" directory',
    'A one-page design and hosting guide, please. Output it in a new "new-design" directory',
    'Give me a deployment manual. Output it in a new "new-design" directory',
    'Summarize the account setup instructions. Output it in a new "new-design" directory',
  ]) {
    let calls = 0;
    await assert.rejects(runBuiltinAsk(ask, { cwd: root, repoRoot, config: localConfig, env,
      log: () => {}, errorOutput: { write() {} },
      runTestCommand: () => assert.fail('Documentation request must not execute deployment'),
      fetchImpl: async (_url, request) => {
        calls += 1;
        assert.match(JSON.parse(request.body).messages[0].content, /builtin planner seat/);
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({ ...directoryPlan,
            files_allowed: ['new-design/deploy.sh'], acceptance_checks: ['npm run deploy succeeds'] }),
        } }] });
      } }), /Planner turn budget.*Markdown documents/);
    assert.equal(calls, 2);
  }
});

test('public directory handoff preserves raw captured whitespace through planner, estimate and critic', async (t) => {
  const { root, repoRoot, env } = directoryFixture(t);
  const raw = ` \t\r\n${directoryAsk.replace('. Output', '.\r\nOutput')}\r\n \t\r\n`;
  const original = raw.replace(/\r\n/g, '\n');
  let calls = 0;
  const result = await runBuiltinAsk(raw, { cwd: root, repoRoot, config: localConfig, env, confirm: true,
    log: () => {}, errorOutput: { write() {} },
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.match(body.messages[0].content, /builtin planner seat/);
      assert.ok(body.messages[1].content.startsWith(original));
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: JSON.stringify(directoryPlan),
      } }] });
    } });
  assert.equal(calls, 1);
  assert.equal(result.confirmedPause, true);
  assert.equal(result.ask, original);
  assert.equal(readFileSync(result.assignmentPath, 'utf8'), `# Local Ask\n\n${original}\n`);
  assert.equal(parseTaskDocument(readFileSync(result.taskPath, 'utf8'), { expectedAsk: raw }).ask, original);
  assert.ok(existsSync(path.join(result.worktreePath, 'ESTIMATE.md')));
  assert.deepEqual(result.planner.critic.defects, []);
  assert.equal(result.runs.coder, null);
  assert.equal(result.runs.reviewer, null);
});

test('public handoff rejects tool-written directory TASKs with trimmed, case-changed or respaced original Ask', async (t) => {
  const { root, repoRoot, env } = directoryFixture(t);
  const raw = ` \t\n${directoryAsk}\n \t\n`;
  const task = buildPlan(raw, { reference: 'local:fixture', title: directoryPlan.title,
    filesAllowed: directoryPlan.files_allowed, acceptanceChecks: directoryPlan.acceptance_checks }).task;
  for (const changed of [task.replace(raw, raw.trim()), task.replace('Security', 'security'),
    task.replace('one page', 'one  page')]) {
    let calls = 0;
    await assert.rejects(runBuiltinAsk(raw, { cwd: root, repoRoot, config: localConfig, env,
      log: () => {}, errorOutput: { write() {} },
      runTestCommand: () => assert.fail('Changed asks must not reach test execution'),
      fetchImpl: async (_url, request) => {
        calls += 1;
        assert.match(JSON.parse(request.body).messages[0].content, /builtin planner seat/);
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: `changed-ask-${calls}`, type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'TASK.md', content: changed }),
          } }],
        } }] });
      } }), /unchanged original Ask/);
    assert.ok(calls > 0);
  }
});

test('public directory handoff retains non-document implementation intent and concrete code files', async (t) => {
  const { root, repoRoot, env } = directoryFixture(t);
  const ask = 'Write a script with setup instructions. Output it in a new "scripts" directory';
  const result = await runBuiltinAsk(ask, { cwd: root, repoRoot, config: localConfig, env, confirm: true,
    log: () => {}, errorOutput: { write() {} },
    fetchImpl: async (_url, request) => {
      assert.match(JSON.parse(request.body).messages[0].content, /builtin planner seat/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: JSON.stringify({ title: 'Script implementation',
          files_allowed: ['scripts/task.mjs'], acceptance_checks: ['node --test exits 0',
            'The script implements the requested behavior'], task_class: 'feat' }),
      } }] });
    } });
  assert.equal(result.askKind, 'slice');
  assert.equal(result.confirmedPause, true);
  assert.deepEqual(parseTaskDocument(result.planner.task, { expectedAsk: ask }).files_allowed, ['scripts/task.mjs']);
});

test('directory-scoped public coder cannot expand to a harmless outside document', async (t) => {
  const { root, repoRoot, env } = directoryFixture(t);
  let worktree;
  let coderCalls = 0;
  await assert.rejects(runBuiltinAsk(directoryAsk, { cwd: root, repoRoot, config: localConfig, env, log: () => {},
    errorOutput: { write() {} }, onPrepared: (prepared) => { worktree = prepared.worktreePath; },
    fetchImpl: async (_url, request) => {
      const system = JSON.parse(request.body).messages[0].content;
      if (system.includes('builtin planner seat')) return Response.json({ choices: [{ finish_reason: 'stop',
        message: { role: 'assistant', content: JSON.stringify(directoryPlan) } }] });
      coderCalls += 1;
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{
        id: 'outside', type: 'function', function: { name: 'write_file',
          arguments: JSON.stringify({ path: 'outside.md', content: '# Unauthorized\n' }) },
      }] } }] });
    } }), /not allowed/);
  assert.equal(coderCalls, 1);
  assert.equal(existsSync(path.join(worktree, 'outside.md')), false);
});

test('cached directory handoff rejects symlink destinations before coder dispatch', async (t) => {
  const { root } = directoryFixture(t);
  const outside = path.join(root, 'outside');
  const worktree = path.join(root, 'worktree');
  mkdirSync(outside);
  mkdirSync(worktree);
  symlinkSync(outside, path.join(worktree, 'new-design'), process.platform === 'win32' ? 'junction' : 'dir');
  const plan = buildPlan(directoryAsk, { reference: 'local:fixture', title: directoryPlan.title,
    filesAllowed: directoryPlan.files_allowed, acceptanceChecks: directoryPlan.acceptance_checks });
  writeFileSync(path.join(worktree, 'RECIPE.yml'), plan.recipe);
  writeFileSync(path.join(worktree, 'TASK.md'), plan.task);
  assert.equal((await readPlannerHandoff({ worktree, ask: directoryAsk, reference: 'local:fixture' })).plan, null);
});

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
