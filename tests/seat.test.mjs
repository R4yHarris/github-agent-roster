import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { runBuiltinTask } from '../src/lib/builtin.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { createTools } from '../src/runtime/tools.mjs';

const sourceRoot = fileURLToPath(new URL('../', import.meta.url));
const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const stub = parseConfig(example);
const configured = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: config-model'));
const sequence = ['principal', 'context', 'research', 'skills', 'tool_loop', 'memory', 'excellence', 'result'];

function fixture(context) {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-seat-'));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  const repoRoot = path.join(root, 'roster');
  const worktree = path.join(root, 'task');
  mkdirSync(worktree);
  for (const directory of ['principals', 'skills']) {
    cpSync(path.join(sourceRoot, directory), path.join(repoRoot, directory), { recursive: true });
  }
  mkdirSync(path.join(repoRoot, 'vendor', 'github-agent-contracts', 'scripts'), { recursive: true });
  writeFileSync(path.join(repoRoot, 'vendor', 'github-agent-contracts', 'scripts', 'agent-pr.mjs'),
    'throw new Error("Standalone coder must not publish");\n');
  writeFileSync(path.join(repoRoot, 'roster.config.example.yml'), example);
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nStay scoped.\n');
  const taskText = planStub('Update README.md.', { title: 'Add a Status section', metadata: { task_class: 'feat', difficulty: 4 } }).task;
  writeFileSync(path.join(worktree, 'TASK.md'), taskText);
  writeFileSync(path.join(worktree, 'README.md'), '# Before\n');
  return { root, repoRoot, worktree, taskText, task: 'issue-42', session: 'coder-42', env: {},
    vault: { get: async () => undefined } };
}

test('standalone slice stub uses minimum stages without a planner, source diff, test, or publication', async (context) => {
  const options = fixture(context);
  const result = await runBuiltinTask({
    ...options, cwd: options.worktree, config: stub, log: () => {},
    fetchImpl: () => assert.fail('Stub must not contact a model'),
    runTestCommand: () => assert.fail('Stub must not run tests'),
  });
  assert.equal(result.askKind, 'slice');
  assert.deepEqual(result.result.stages, ['context', 'skills', 'tool_loop', 'memory', 'excellence', 'result']);
  assert.equal(result.result.mode, 'stub');
  assert.equal(result.result.excellence.pass, false);
  assert.equal(readFileSync(path.join(options.worktree, 'TASK.md'), 'utf8'), options.taskText);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Before\n');
  assert.deepEqual(readdirSync(options.worktree).sort(),
    ['.roster', 'AGENTS.md', 'CONTEXT.md', 'README.md', 'RESULT.md', 'REVIEW.md', 'TASK.md']);
  const liveLog = readFileSync(result.logPath, 'utf8');
  assert.match(liveLog, /start seat coder[\s\S]*mode stub[\s\S]*wrote RESULT\.md/);
  assert.match(liveLog, /start seat reviewer[\s\S]*wrote REVIEW\.md[\s\S]*elapsed_ms=\d+ mode=stub/);
  assert.equal(existsSync(path.join(options.worktree, '.roster', 'runs', 'runs.jsonl')), false);
  assert.equal(result.review.verdict, 'fail');
  assert.match(readFileSync(result.review.reviewPath, 'utf8'), /Verdict: fail/);
  assert.match(readFileSync(result.result.resultPath, 'utf8'), /Add a Status section/);
  assert.match(readFileSync(result.result.resultPath, 'utf8'), /Stages: context -> skills -> tool_loop -> memory -> excellence -> result/);
  assert.equal(existsSync(path.join(options.repoRoot, '.roster', 'memory', 'planner.jsonl')), false);
});

test('configured standalone logging does not become an app diff or opt into JSONL metrics', async (context) => {
  const options = fixture(context);
  let toolTurns = 0;
  let stderr = '';
  const result = await runBuiltinTask({
    ...options, cwd: options.worktree, config: configured, log: () => {},
    errorOutput: { write(text) { stderr += String(text); } },
    env: { ROSTER_API_KEY: 'test-only-live-key' },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      if (body.messages[0].content.startsWith('You are the builtin research step.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: 'PRIVATE_RESEARCH_RESPONSE',
        } }] });
      }
      toolTurns += 1;
      return Response.json({ choices: [toolTurns === 1 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'write', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Before\n\n## Status\nReady.\n' }),
        } }],
      } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'PRIVATE_CODER_COMPLETION' } }] });
    },
    runTestCommand: async () => ({ stdout: 'PRIVATE_TEST_OUTPUT', stderr: '' }),
  });
  assert.equal(toolTurns, 2);
  assert.equal(result.result.mode, 'llm');
  assert.equal(result.result.excellence.pass, true);
  assert.deepEqual(result.result.excellence.files, ['README.md']);
  const liveLog = readFileSync(result.logPath, 'utf8');
  assert.notEqual(liveLog, stderr);
  assert.match(stderr, /^Saving README\.md\.$/m);
  assert.match(liveLog, /seat coder tool write_file path="README\.md"/);
  assert.match(liveLog, /seat coder http chat\.completions ok status=200/);
  assert.match(liveLog, /model="config-model" host="localhost:3456"/);
  assert.doesNotMatch(stderr, /http chat|model=|host=|elapsed_ms=|\d{4}-\d\d-\d\dT/);
  assert.doesNotMatch(stderr, /PRIVATE_|test-only-live-key|# Before|## Status/);
  assert.equal(existsSync(path.join(options.worktree, '.roster', 'runs', 'runs.jsonl')), false);
});

test('the exact standalone CLI command consumes TASK.md in cwd with no GitHub dependency', (context) => {
  const options = fixture(context);
  cpSync(path.join(sourceRoot, 'src'), path.join(options.repoRoot, 'src'), { recursive: true });
  const env = { ...process.env, AI_TASK: 'local-task', AI_SESSION: 'single-seat-test',
    GITHUB_AGENT_CONTRACTS: path.join(options.repoRoot, 'vendor', 'github-agent-contracts'),
    ROSTER_MODEL: '' };
  for (const name of Object.keys(env)) if (name.toLowerCase() === 'path') delete env[name];
  env.PATH = '';
  const result = spawnSync(process.execPath, [
    path.join(options.repoRoot, 'src', 'cli.mjs'), 'run', '--seat', 'coder', '--runtime', 'builtin',
  ], { cwd: options.worktree, env, encoding: 'utf8', timeout: 10_000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Mode: stub/);
  assert.match(result.stdout, /CONTEXT: .+CONTEXT\.md/);
  assert.doesNotMatch(result.stdout, /RESEARCH:|undefined/);
  assert.match(result.stderr, /^Preparing the task summary\.$/m);
  assert.match(result.stderr, /^Checking the diff against the task\.$/m);
  assert.doesNotMatch(result.stderr, /\d{4}-\d\d-\d\dT|start seat|mode stub/);
  const liveLog = readFileSync(path.join(options.worktree, '.roster', 'runs', 'single-seat-test.log'), 'utf8');
  assert.notEqual(liveLog, result.stderr);
  assert.match(liveLog, /start seat coder[\s\S]*seat coder wrote RESULT\.md/);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Before\n');
  assert.equal(existsSync(path.join(options.worktree, 'RECIPE.yml')), false);
  for (const flag of ['--publish', '--auto-model']) {
    const invalid = spawnSync(process.execPath, [
      path.join(options.repoRoot, 'src', 'cli.mjs'), 'run', '--seat', 'coder', '--runtime', 'builtin', flag,
    ], { cwd: options.worktree, env, encoding: 'utf8', timeout: 10_000 });
    assert.equal(invalid.status, 1);
    assert.match(invalid.stderr, /Use roster run/);
  }
});

test('standalone broad TASK is classified before coder and produces PLAN without changing README or TASK', async (context) => {
  const options = fixture(context);
  const task = planStub('build an orchestrator.\n\n## Allowed files\n- `README.md`').task;
  writeFileSync(path.join(options.worktree, 'TASK.md'), task);
  const result = await runBuiltinTask({ ...options, cwd: options.worktree, config: stub, log: () => {},
    fetchImpl: () => assert.fail('Initiative stub cannot call a model'),
    runTestCommand: () => assert.fail('Initiative must not call coder tests'),
  });
  assert.equal(result.askKind, 'initiative');
  assert.equal(result.planningOnly, true);
  assert.equal(result.result, undefined);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Before\n');
  assert.equal(readFileSync(path.join(options.worktree, 'TASK.md'), 'utf8'), task);
  assert.match(readFileSync(result.planPath, 'utf8'), /Ask kind: initiative[\s\S]*## Waves/);
  assert.doesNotMatch(readFileSync(result.logPath, 'utf8'), /start seat coder|start seat reviewer/);
  assert.equal(existsSync(path.join(options.worktree, 'RESULT.md')), false);
});

test('configured seat respects the task model and materializes artifacts before edits and memory', async (context) => {
  const options = fixture(context);
  writeFileSync(path.join(options.worktree, 'TASK.md'), options.taskText.replace(/^model: *$/m, 'model: task-model'));
  const memoryPath = path.join(options.repoRoot, '.roster', 'memory', 'coder.jsonl');
  let calls = 0;
  const result = await runCoder({
    ...options, config: configured,
    priorFeedback: 'Last human verdict: rework; preserve prior checks.',
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.model, 'task-model');
      if (calls > 1) assert.match(body.messages[0].content, /## Prior feedback[\s\S]*preserve prior checks/);
      assert.equal(existsSync(path.join(options.worktree, 'CONTEXT.md')), true);
      assert.equal(existsSync(path.join(options.worktree, 'RESEARCH.md')), true);
      assert.equal(existsSync(memoryPath), false);
      assert.equal(existsSync(path.join(options.worktree, 'RESULT.md')), false);
      if (calls === 1) {
        assert.equal(body.tools, undefined);
        assert.match(body.messages[0].content, /builtin research step/);
        return { status: 200, json: async () => ({
          choices: [{ message: { role: 'assistant', content: 'Inspect README and add Status.' } }],
          usage: { prompt_tokens: 1, completion_tokens: 1 },
        }) };
      }
      if (calls === 2) return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'write', type: 'function',
            function: { name: 'write_file', arguments: JSON.stringify({
              path: 'README.md', content: '# Before\n\n## Status\nReady.\n',
            }) } }],
        } }], usage: { prompt_tokens: 2, completion_tokens: 1 },
      }) };
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'README updated; verify tests.' } }],
        usage: { prompt_tokens: 3, completion_tokens: 1 },
      }) };
    },
    runTestCommand: async () => {
      assert.match(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), /## Status/);
      assert.equal(existsSync(memoryPath), false);
      assert.equal(existsSync(path.join(options.worktree, 'RESULT.md')), false);
      return { stdout: 'tests pass', stderr: '' };
    },
  });
  assert.deepEqual(result.stages, sequence);
  assert.equal(result.model, 'task-model');
  assert.equal(result.excellence.pass, true);
  assert.match(result.run.line, /task-model@/);
  assert.equal(result.run.env.AI_CONTEXT_USED, '3');
  assert.deepEqual(result.usage, { prompt_tokens: 6, completion_tokens: 3 });
  assert.equal(result.turns, 2);
  assert.equal(result.research.turns, 1);
  assert.equal(JSON.parse(readFileSync(memoryPath, 'utf8')).tests, 'node --test exited 0');
});

test('a post-research skill change fails before writes and saves the partial-stage result', async (context) => {
  const options = fixture(context);
  const result = runCoder({
    ...options, config: configured,
    fetchImpl: async () => {
      writeFileSync(path.join(options.repoRoot, 'skills', 'implement-task', 'SKILL.md'), '# Changed\n');
      return { status: 200, json: async () => ({
        choices: [{ message: { role: 'assistant', content: 'Research completed.' } }],
      }) };
    },
    runTestCommand: () => assert.fail('No tests before the skill handoff succeeds'),
  });
  await assert.rejects(result, /Task skills changed after the context pack/);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Before\n');
  assert.match(readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8'),
    /Checks: FAIL[\s\S]*Stages: principal -> context -> research -> memory -> excellence -> result/);
});

test('a failing notebook append still writes an explicit result without rewriting the notebook', async (context) => {
  const options = fixture(context);
  const memoryPath = path.join(options.repoRoot, '.roster', 'memory', 'coder.jsonl');
  mkdirSync(path.dirname(memoryPath), { recursive: true });
  writeFileSync(memoryPath, '{"summary":"incomplete"}');
  await assert.rejects(runCoder({ ...options, config: stub }), /incomplete JSONL/);
  assert.equal(readFileSync(memoryPath, 'utf8'), '{"summary":"incomplete"}');
  assert.match(readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8'),
    /Checks: FAIL[\s\S]*Memory append failed/);
});

test('a custom notebook inside the worktree is managed rather than an app-code diff', async (context) => {
  const options = fixture(context);
  for (const file of ['AGENTS.md', 'TASK.md', 'README.md']) {
    cpSync(path.join(options.worktree, file), path.join(options.repoRoot, file));
  }
  const config = parseConfig(example.replace('.roster/memory/coder.jsonl', 'custom/history.jsonl'));
  const result = await runCoder({ ...options, worktree: options.repoRoot, config });
  assert.deepEqual(result.excellence.files, []);
  assert.equal(result.memoryPath, path.join(options.repoRoot, 'custom', 'history.jsonl'));
  assert.equal(JSON.parse(readFileSync(result.memoryPath, 'utf8')).status, 'stub');
  const tools = await createTools({
    worktree: options.repoRoot, allowedFiles: ['**/*'], memoryPath: result.memoryPath,
  });
  await assert.rejects(tools.write_file({ path: 'custom/history.jsonl', content: 'tampered' }), /not allowed/);
});

test('ambiguous test waivers and invalid standalone IDs fail before a model request', async (context) => {
  const options = fixture(context);
  writeFileSync(path.join(options.worktree, 'TASK.md'),
    options.taskText.replace('---\n', '---\ntests: none\ntests: required\n'));
  await assert.rejects(runCoder({
    ...options, config: configured, fetchImpl: () => assert.fail('Invalid task must not contact a model'),
  }), /must not repeat tests/);
  assert.match(readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8'), /Checks: FAIL/);
  await assert.rejects(runBuiltinTask({
    ...options, cwd: options.worktree, config: stub, task: '../invalid', log: () => {},
  }), /opaque 1-64 character identifiers/);
});
