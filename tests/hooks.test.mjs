import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { hookEnvironment, parseHooks, requireLifecycleHooks, runLifecycleHooks } from '../src/runtime/hooks.mjs';
import { createTools, isForbiddenWrite } from '../src/runtime/tools.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { fixture as builtinFixture, git, llmConfig, runBuiltinIssue, stubConfig } from './helpers/builtin.mjs';
import { prepareBuiltinPublication } from '../src/lib/builtin.mjs';
import { runLoop } from '../src/runtime/loop.mjs';
import { planStub } from '../src/planner/stub.mjs';

function install(cwd, source, event = 'post-coder', timeout = 10000) {
  mkdirSync(path.join(cwd, '.roster', 'hooks'), { recursive: true });
  writeFileSync(path.join(cwd, '.roster', 'hooks.yml'),
    `hooks:\n  - event: ${event}\n    script: .roster/hooks/check.mjs\n    timeout_ms: ${timeout}\n`);
  writeFileSync(path.join(cwd, '.roster', 'hooks', 'check.mjs'), source);
  git(cwd, 'add', '--', '.roster/hooks.yml', '.roster/hooks/check.mjs');
  git(cwd, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'Human-reviewed hook fixture');
}

function fixture(t, source = '', event, timeout) {
  const cwd = mkdtempSync(path.join(tmpdir(), 'roster-hooks-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  git(cwd, 'init', '-b', 'main');
  writeFileSync(path.join(cwd, 'README.md'), '# Example\n');
  install(cwd, source, event, timeout);
  return cwd;
}

test('minimal hook YAML is strict, portable and bounded', () => {
  assert.deepEqual(parseHooks('hooks: []\n'), []);
  assert.deepEqual(parseHooks('hooks:\r\n  - event: pre-plan\r\n    script: ".roster/hooks/check.mjs"\r\n'), [
    { event: 'pre-plan', script: '.roster/hooks/check.mjs', timeout_ms: 10000 },
  ]);
  for (const extra of ['    command: node\n', '    timeout_ms: 0\n', '    timeout_ms: 60001\n']) {
    assert.throws(() => parseHooks(`hooks:\n  - event: post-coder\n    script: .roster/hooks/check.mjs\n${extra}`));
  }
  for (const script of ['../outside.mjs', '.roster/hooks/../outside.mjs', 'check.sh', '.roster/hooks/check.mjs;echo']) {
    assert.throws(() => parseHooks(`hooks:\n  - event: pre-publish\n    script: "${script}"\n`), /Node script/);
  }
  assert.throws(() => parseHooks('hooks:\n  - event: post-review\n    script: .roster/hooks/check.mjs\n'), /event/);
});

test('no hooks changes nothing; successful hooks contribute no findings', async (t) => {
  const cwd = fixture(t, 'console.log("successful private diagnostic");');
  const good = await runLifecycleHooks('post-coder', { worktree: cwd });
  assert.equal(good.pass, true);
  assert.deepEqual(good.reasons, []);
  assert.equal(good.entries[0].exit_code, 0);
  assert.deepEqual((await runLifecycleHooks('pre-plan', { worktree: cwd })).entries, []);
  rmSync(path.join(cwd, '.roster', 'hooks.yml'));
  assert.deepEqual(await runLifecycleHooks('pre-plan', { worktree: cwd }), { pass: true, reasons: [], entries: [] });
  rmSync(path.join(cwd, '.roster'), { recursive: true });
  writeFileSync(path.join(cwd, '.roster'), 'legacy-regular-state-marker\n');
  assert.deepEqual(await runLifecycleHooks('post-coder', { worktree: cwd }), { pass: true, reasons: [], entries: [] });
});

test('nonzero exit returns last output as a gate finding and pre-publish cannot bypass it', async (t) => {
  const cwd = fixture(t, 'console.log("first line"); console.error("expected Ready status"); process.exitCode = 1;', 'pre-publish');
  const result = await runLifecycleHooks('pre-publish', { worktree: cwd });
  assert.equal(result.pass, false);
  assert.match(result.reasons[0], /Lifecycle hook: pre-publish .*failed \(exit 1\).*first line/s);
  assert.match(result.reasons[0], /expected Ready status/);
  await assert.rejects(requireLifecycleHooks('pre-publish', { worktree: cwd }), /expected Ready status/);
});

test('timeout terminates the owned hook and reports a bounded timeout finding', async (t) => {
  const cwd = fixture(t, 'setInterval(() => {}, 1000);', 'post-coder', 100);
  const started = performance.now();
  const result = await runLifecycleHooks('post-coder', { worktree: cwd });
  assert.equal(result.pass, false);
  assert.equal(result.entries[0].status, 'timeout');
  assert.match(result.reasons[0], /timed out after 100 ms/);
  assert.ok(performance.now() - started < 10000);
});

test('timeout kills an owned child process tree rather than leaving a worker running', async (t) => {
  const cwd = fixture(t, `import { spawn } from "node:child_process";
    const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: "ignore" });
    console.log(child.pid); setInterval(() => {}, 1000);`, 'post-coder', 1500);
  const result = await runLifecycleHooks('post-coder', { worktree: cwd });
  const pid = Number(result.entries[0].output.trim());
  assert.ok(Number.isSafeInteger(pid) && pid > 0, 'fixture must actually spawn its worker');
  assert.equal(result.entries[0].status, 'timeout');
  if (process.platform === 'win32') assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
});

test('overflow stops output rather than returning an unbounded or partial secret diagnostic', async (t) => {
  const cwd = fixture(t, 'process.stdout.write("x".repeat(5000)); setInterval(() => {}, 1000);');
  const result = await runLifecycleHooks('post-coder', { worktree: cwd });
  assert.equal(result.entries[0].status, 'output-limit');
  assert.equal(result.entries[0].output, '');
  assert.match(result.reasons[0], /4 KiB output limit/);
});

test('hook child has only allow-listed environment, no App/model/Git/node preload credentials', async (t) => {
  const cwd = fixture(t, `const forbidden = ["GITHUB_APP_ID", "GITHUB_APP_PRIVATE_KEY_PATH", "GH_TOKEN",
    "GITHUB_TOKEN", "ROSTER_API_KEY", "CUSTOM_AUTH", "NODE_OPTIONS", "GIT_CONFIG_COUNT", "HOME"];
    if (forbidden.some(key => process.env[key])) { console.error("credential leak"); process.exitCode = 1; }
    console.error("seed-private-value");`);
  const env = { ...process.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'private-path',
    GH_TOKEN: 'token-fixture', GITHUB_TOKEN: 'token-fixture', ROSTER_API_KEY: 'seed-private-value',
    CUSTOM_AUTH: 'unrecognizable-key', NODE_OPTIONS: '--require absent.cjs', GIT_CONFIG_COUNT: '1', HOME: 'private-home' };
  const result = await runLifecycleHooks('post-coder', { worktree: cwd, env });
  assert.equal(result.pass, true, result.reasons.join('\n'));
  assert.doesNotMatch(JSON.stringify(result), /seed-private-value/);
  assert.deepEqual(hookEnvironment({ PATH: 'p', CUSTOM_AUTH: 'x', NODE_OPTIONS: 'x' }), { PATH: 'p' });
});

test('uncommitted config/scripts, symlinks and mutations are refused', async (t) => {
  const cwd = fixture(t, '');
  const script = path.join(cwd, '.roster', 'hooks', 'check.mjs');
  writeFileSync(script, 'throw new Error("unreviewed");');
  await assert.rejects(runLifecycleHooks('post-coder', { worktree: cwd }), /differs from committed HEAD/);
  rmSync(path.dirname(script), { recursive: true });
  symlinkSync(cwd, path.dirname(script), process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(runLifecycleHooks('post-coder', { worktree: cwd }), /symlinks/);
  const uncommitted = mkdtempSync(path.join(tmpdir(), 'roster-untrusted-hooks-'));
  t.after(() => rmSync(uncommitted, { recursive: true, force: true }));
  mkdirSync(path.join(uncommitted, '.roster', 'hooks'), { recursive: true });
  writeFileSync(path.join(uncommitted, '.roster', 'hooks.yml'),
    'hooks:\n  - event: pre-plan\n    script: .roster/hooks/check.mjs\n');
  writeFileSync(path.join(uncommitted, '.roster', 'hooks', 'check.mjs'), '');
  await assert.rejects(runLifecycleHooks('pre-plan', { worktree: uncommitted }), /human-reviewed and committed/);
});

test('hook side effects fail explicitly and remain visible, never silently rolled back', async (t) => {
  const cwd = fixture(t, 'import { writeFileSync } from "node:fs"; writeFileSync("README.md", "unexpected mutation");');
  const result = await runLifecycleHooks('post-coder', { worktree: cwd });
  assert.equal(result.pass, false);
  assert.match(result.reasons[0], /changed worktree files/);
  assert.equal(readFileSync(path.join(cwd, 'README.md'), 'utf8'), 'unexpected mutation');
});

test('staging by a hook is a hard failure even without changing any product contents', async (t) => {
  const cwd = fixture(t, 'import { execFileSync } from "node:child_process"; execFileSync("git", ["add", "README.md"]);');
  const result = await runLifecycleHooks('post-coder', { worktree: cwd });
  assert.equal(result.pass, false);
  assert.match(result.reasons[0], /changed worktree files or Git state/);
});

test('unresolved hook finding gets exactly one loop correction and still blocks completion', async () => {
  const task = planStub('Update README.md.').task;
  let requests = 0;
  let gates = 0;
  let seenCorrection = false;
  const result = await runLoop({
    config: llmConfig, context: { task, pack: task }, env: {},
    tools: { write_file: async () => ({ path: 'README.md' }), run_test: async () => ({ exit_code: 0 }) },
    fetchImpl: async (_url, request) => {
      requests += 1;
      assert.ok(requests <= 5, 'hook repairs must not loop indefinitely');
      const body = JSON.parse(request.body);
      const correction = body.messages.some(({ role, content }) => role === 'user' &&
        typeof content === 'string' && content.includes('One lifecycle-hook correction is allowed.'));
      seenCorrection ||= correction;
      const message = requests === 1 || correction && requests === 3
        ? { role: 'assistant', tool_calls: [{ id: `write-${requests}`, type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
        } }] }
        : { role: 'assistant', content: 'Done.' };
      return Response.json({ choices: [{ finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message }] });
    },
    verify: () => { gates += 1; return { pass: false, reasons: ['Lifecycle hook: post-coder check.mjs failed (exit 1): unmet requirement'] }; },
  });
  assert.match(result.error?.message ?? '', /Lifecycle hook/);
  assert.equal(seenCorrection, true);
  assert.equal(gates, 2);
});

test('cancellation kills the owned hook and propagates cancellation, not a failed gate', async (t) => {
  const cwd = fixture(t, 'setInterval(() => {}, 1000);');
  const controller = new AbortController();
  const running = runLifecycleHooks('post-coder', { worktree: cwd, signal: controller.signal });
  const timer = setTimeout(() => controller.abort(), 300);
  try {
    await assert.rejects(running, { code: 'ROSTER_CANCELLED' });
  } finally {
    clearTimeout(timer);
  }
});

test('coder write/read/edit/delete tools refuse manifest and scripts even when allow-listed', async (t) => {
  const cwd = fixture(t);
  const files = ['.roster/hooks.yml', '.roster/hooks/check.mjs'];
  const tools = await createTools({ worktree: cwd, allowedFiles: files });
  for (const file of files) {
    assert.equal(isForbiddenWrite(file), true);
    await assert.rejects(tools.read_file({ path: file }));
    await assert.rejects(tools.write_file({ path: file, content: 'bypass' }));
    await assert.rejects(tools.edit_file({ path: file, old_text: 'hooks', new_text: 'bypass' }));
    await assert.rejects(tools.delete_file({ path: file }));
  }
  assert.equal(isForbiddenWrite('.ROSTER\\HOOKS.YML'), true);
});

test('pre-plan failure blocks the planner before any model request', async (t) => {
  const options = builtinFixture(t);
  install(options.target, 'console.error("missing prerequisite"); process.exitCode = 1;', 'pre-plan');
  await assert.rejects(runBuiltinIssue(42, { ...options, config: llmConfig,
    fetchImpl: () => assert.fail('Hook must block model requests'), log() {} }), /missing prerequisite/);
});

test('post-coder finding returns to the coder for one correction and the repaired hook passes', async (t) => {
  const options = builtinFixture(t);
  install(options.target, `import { readFileSync } from "node:fs";
    if (!readFileSync("README.md", "utf8").includes("Ready.")) {
      console.error("expected Ready status"); process.exitCode = 1;
    }`);
  let requests = 0;
  let repaired = false;
  const reply = (message) => ({ status: 200, json: async () => ({
    choices: [{ finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message }],
    model: 'local-model', usage: { prompt_tokens: 8, completion_tokens: 3 },
  }) });
  const run = await runBuiltinIssue(42, { ...options, config: llmConfig, log() {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      requests += 1;
      if (requests === 1) return reply({ role: 'assistant', content: JSON.stringify({
        title: options.issue.title, acceptance_checks: ['README has a Status section'], files_allowed: ['README.md'],
      }) });
      if (body.messages.some((message) => message.role === 'user' && message.content.includes('expected Ready status'))) {
        repaired = true;
      }
      if (requests === 2 || repaired && body.messages.at(-1).role === 'user') return reply({
        role: 'assistant', tool_calls: [{ id: `write-${requests}`, type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'README.md',
            content: `# Example\n\n## Status\n${repaired ? 'Ready.' : 'Pending.'}\n` }),
        } }],
      });
      return reply({ role: 'assistant', content: 'Updated README status.' });
    } });
  assert.equal(repaired, true);
  assert.equal(run.failed, false, run.error?.message);
  assert.equal(run.result.excellence.pass, true, run.result.excellence.reasons.join('\n'));
  assert.equal(run.result.hooks.pass, true);
  assert.match(readFileSync(run.result.resultPath, 'utf8'), /## Lifecycle hooks/);
  const publishOptions = { cwd: options.cwd, config: llmConfig, skipReview: true,
    env: { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'fixture.pem' } };
  // Change the fixture's human-owned manifest at a committed boundary, not through the coder tools.
  install(run.worktreePath, 'console.error("reviewed publish prerequisite"); process.exitCode = 1;', 'pre-publish');
  await assert.rejects(prepareBuiltinPublication(run, publishOptions), /reviewed publish prerequisite/);
});

test('manual GHCP publication runs pre-publish hooks before invoking the publisher', async (t) => {
  const cwd = fixture(t, 'console.error("manual publish blocked"); process.exitCode = 1;', 'pre-publish');
  const shell = createDispatcher({ cwd, config: stubConfig,
    env: { ...process.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'fixture.pem' },
    output: { write() {} }, errorOutput: { write() {} }, services: {
      repositoryRoot: () => cwd, publisher: () => assert.fail('Blocked hook must not publish'),
      resolveContractsPath: () => assert.fail('Hook must block before publisher resolution'),
    } });
  await assert.rejects(shell.dispatch('/publish --model GPT-6.1-Sol --skip-review feat: fixture'), /manual publish blocked/);
});
