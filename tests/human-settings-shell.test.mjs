import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { formatHelp } from '../src/shell/commands.mjs';
import { parseShellEvaluationArgs } from '../src/shell/evaluation.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example);

function fixture(t, source = example, env = {}, services = {}) {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-human-shell-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(path.join(root, '.roster'));
  writeFileSync(path.join(root, 'roster.config.example.yml'), example);
  const file = path.join(root, '.roster', 'config.yml');
  writeFileSync(file, source);
  let text = '';
  const shell = createDispatcher({ cwd: root, repoRoot: root, config: parseConfig(source), env,
    output: { write(value) { text += value; } }, errorOutput: { write(value) { text += value; } },
    services: { repositoryRoot: () => root, repositoryBranch: () => 'main', ...services } });
  return { root, file, ...shell, get text() { return text; }, reset() { text = ''; } };
}

test('new eval flags feed the existing human writer and help names minutes and difficulty', async () => {
  const parsed = parseShellEvaluationArgs('abcdef12 accept --minutes 18 --difficulty 2 "Meets the  checks."');
  assert.deepEqual(parsed, { values: ['abcdef12', 'accept', '2', 'n'],
    options: { minutes: 18, comment: 'Meets the  checks.' } });
  assert.match(formatHelp('eval'), /--minutes N/);
  assert.match(formatHelp('eval'), /--difficulty 1-5/);
  let received;
  const shell = createDispatcher({ config, env: {}, services: {
    repositoryBranch: () => 'main', repositoryRoot: () => process.cwd(),
    recordEvaluation: async (...args) => { received = args; return { sha: args[0] }; },
  }, output: { write() {} }, errorOutput: { write() {} } });
  await shell.dispatch('/eval abcdef12 rework --difficulty 4 --minutes 30 "Keep the regression test."');
  assert.deepEqual(received.slice(0, 4), ['abcdef12', 'rework', '4', 'n']);
  assert.equal(received[4].minutes, 30);
  assert.equal(received[4].comment, 'Keep the regression test.');
  for (const text of ['abcdef12 accept --minutes 2 "missing difficulty"',
    'abcdef12 accept --minutes 2 --difficulty 6 "bad"',
    'abcdef12 accept --minutes 2 --difficulty 2 --minutes 4']) {
    assert.throws(() => parseShellEvaluationArgs(text), /Use \/eval/);
  }
});

test('an agent seat cannot invoke even an injected human eval writer', async () => {
  const shell = createDispatcher({ config, env: { ROSTER_SEAT: 'coder' },
    output: { write() {} }, errorOutput: { write() {} }, services: {
      repositoryBranch: () => 'main', recordEvaluation: () => assert.fail('Agents must not evaluate themselves'),
    } });
  await assert.rejects(shell.dispatch('/eval abcdef12 accept --minutes 1 --difficulty 1 "Good"'), /human-only/);
});

test('config display omits secret material and PEM paths; path prints only the private config path', async (t) => {
  const source = example.replace('model: ""', 'model: "config-secret-marker"')
    .replace(/^  memory: .*$/m, '  memory: ".roster/private-key.pem/memory.jsonl"') +
    '\n# password: private-comment-marker\n';
  const shell = fixture(t, source, { ROSTER_API_KEY: 'config-secret-marker' });
  await shell.dispatch('/config');
  assert.match(shell.text, /schema: 1/);
  assert.match(shell.text, /\[redacted\]/);
  assert.doesNotMatch(shell.text, /config-secret-marker|private-key\.pem|private-comment-marker/);
  shell.reset();
  await shell.dispatch('/config path');
  assert.equal(shell.text, `${shell.file}\n`);
});

test('config set is narrow, rejects PEM/endpoints/policy, and preserves a session model', async (t) => {
  const shell = fixture(t);
  await shell.dispatch('/model session-model');
  await shell.dispatch('/config set effort l');
  assert.equal(shell.state.config.llm.model, 'session-model');
  assert.equal(shell.state.config.llm.effort_override, 'l');
  assert.equal(parseConfig(readFileSync(shell.file, 'utf8')).llm.model, '');
  assert.equal(parseConfig(readFileSync(shell.file, 'utf8')).llm.effort_override, 'l');
  const before = readFileSync(shell.file, 'utf8');
  for (const command of ['/config set model other', '/config set llm.base_url https://example.invalid',
    '/config set effort C:\\private\\key.pem', '/config set agent-policy.yml allow',
    '/config set context.budget 0']) {
    await assert.rejects(shell.dispatch(command), /Config set|Config context/);
  }
  assert.equal(readFileSync(shell.file, 'utf8'), before);
  await shell.dispatch('/config set debug on');
  await shell.dispatch('/config set statusbar false');
  assert.equal(shell.state.debug.enabled, true);
  assert.equal(shell.state.statusbar, false);
  assert.equal(readFileSync(shell.file, 'utf8'), before);
});

test('context budget updates both seat context and the legacy context alias', async (t) => {
  const source = example + '\ncontext:\n  budget: 4000\n';
  const shell = fixture(t, source);
  await shell.dispatch('/config set context.budget 12000');
  const saved = parseConfig(readFileSync(shell.file, 'utf8'));
  assert.equal(saved.context.budget, 12000);
  assert.equal(saved.seat.context_chars, 12000);
  assert.equal(shell.state.config.seat.context_chars, 12000);
  assert.equal(shell.state.config.context.budget, 12000);
});

test('recommend without args uses last task metadata and changes no session or saved default', async (t) => {
  let requested;
  const shell = fixture(t, example, {}, {
    loadAvailableMetrics: () => [],
    routeTask: async (options) => { requested = options; return null; },
  });
  shell.state.lastRun = { repoRoot: shell.root, planner: { metadata: { task_class: 'fix', difficulty: 4 } } };
  const before = readFileSync(shell.file, 'utf8');
  const active = shell.state.config;
  await shell.dispatch('/recommend');
  assert.equal(requested.taskClass, 'fix');
  assert.equal(requested.difficulty, 4);
  assert.equal(shell.state.config, active);
  assert.equal(readFileSync(shell.file, 'utf8'), before);
});

test('doctor uses the existing checks and warm prints only host/status, not its raw diagnostic stream', async (t) => {
  const source = example.replace('base_url: ""', 'base_url: http://localhost:8000/v1');
  let checked = false;
  const shell = fixture(t, source, { GITHUB_APP_ID: 'secret-app-marker' }, {
    checkDoctor: () => {
      checked = true;
      return { checks: Array.from({ length: 6 }, (_, index) => ({ name: `Check ${index}`, ok: true })) };
    },
    warmDoctor: async (options) => {
      assert.ok(options.signal instanceof AbortSignal);
      options.errorOutput.write('PRIVATE_WARM_DIAGNOSTIC');
      return { status: 200 };
    },
  });
  await shell.dispatch('/doctor');
  assert.equal(checked, true);
  shell.reset();
  await shell.dispatch('/doctor warm');
  assert.equal(shell.text, 'Warm probe: host=localhost:8000 status=200\n');
  assert.doesNotMatch(shell.text, /PRIVATE_WARM|secret-app-marker|timeout|elapsed/);
});
