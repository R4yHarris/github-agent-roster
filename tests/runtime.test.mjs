import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { appendMemory, readMemory } from '../src/runtime/memory.mjs';
import { loadSkills } from '../src/runtime/skills.mjs';
import { runCoder } from '../src/seats/coder.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const stubConfig = parseConfig(example);
const llmConfig = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 8', 'turn_budget: 3'));

function fixture(context, config = stubConfig) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-runtime-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  const skillDirectory = path.join(repoRoot, 'skills', 'implement-task');
  mkdirSync(worktree);
  mkdirSync(skillDirectory, { recursive: true });
  writeFileSync(path.join(skillDirectory, 'SKILL.md'), '# Implement task\nRun tests.\n');
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nCode carefully.\n');
  writeFileSync(path.join(worktree, 'TASK.md'),
    planStub('Update `README.md` with a Status section.', { reference: 'issue:4' }).task);
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  return {
    repoRoot, worktree, config, task: 'issue-4', session: 'roster-session',
    memoryPath: path.join(repoRoot, config.paths.memory),
  };
}

test('loads worktree context, this roster repository skills, and only the last 20 memory lines', async (context) => {
  const options = fixture(context);
  for (let index = 0; index < 25; index += 1) {
    await appendMemory({ file: options.memoryPath, repoRoot: options.repoRoot, record: { index } });
  }
  const loaded = await loadContext({
    worktree: options.worktree, memoryPath: options.memoryPath, repoRoot: options.repoRoot,
  });
  assert.match(loaded.agents, /Code carefully/);
  assert.match(loaded.task, /Files allowed/);
  assert.equal(loaded.memory.length, 20);
  assert.deepEqual(loaded.memory.map((line) => JSON.parse(line).index),
    Array.from({ length: 20 }, (_, index) => index + 5));
  assert.deepEqual(await loadSkills({ repoRoot: options.repoRoot }), [
    { name: 'implement-task', content: '# Implement task\nRun tests.\n' },
  ]);
  writeFileSync(options.memoryPath, '{bad-json\n');
  await assert.rejects(readMemory({ file: options.memoryPath, repoRoot: options.repoRoot }), /Invalid memory JSONL line 1/);
});

test('stub coder writes a deterministic result without contacting an LLM or running tests', async (context) => {
  const options = fixture(context);
  const result = await runCoder({
    ...options, env: {},
    fetchImpl: () => { throw new Error('stub attempted network access'); },
    runTestCommand: () => { throw new Error('stub attempted to run tests'); },
  });
  assert.equal(result.mode, 'stub');
  assert.equal(result.turns, 0);
  assert.match(result.summary, /no implementation or tests were run/);
  assert.match(readFileSync(result.resultPath, 'utf8'), /Update `README\.md`/);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Example\n');
  assert.deepEqual(JSON.parse((await readMemory({
    file: options.memoryPath, repoRoot: options.repoRoot,
  }))[0]).status, 'stub');
});

test('LLM coder uses only offered tools within the turn budget, then verifies tests', async (context) => {
  const options = fixture(context, llmConfig);
  let calls = 0;
  let tests = 0;
  const fetchImpl = async (url, request) => {
    calls += 1;
    assert.equal(String(url), 'http://localhost:3456/v1/chat/completions');
    assert.equal(request.headers.Authorization, 'Bearer private-value');
    const sent = JSON.parse(request.body);
    assert.deepEqual(sent.tools.map((tool) => tool.function.name),
      ['read_file', 'write_file', 'list_dir', 'run_test']);
    if (calls === 1) {
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant',
          content: null,
          tool_calls: [
            { id: 'write-1', type: 'function', function: {
              name: 'write_file', arguments: JSON.stringify({
                path: 'README.md', content: '# Example\n\n## Status\nReady.\n',
              }),
            } },
            { id: 'test-1', type: 'function', function: { name: 'run_test', arguments: '{}' } },
          ],
        } }],
        usage: { prompt_tokens: 20, completion_tokens: 8 },
      }) };
    }
    assert.equal(sent.messages.at(-2).tool_call_id, 'write-1');
    assert.equal(sent.messages.at(-1).tool_call_id, 'test-1');
    return { status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: {
        role: 'assistant',
        content: 'Updated README Status; node --test passed.',
      } }],
      usage: { prompt_tokens: 26, completion_tokens: 9 },
    }) };
  };
  const result = await runCoder({
    ...options, env: { ROSTER_API_KEY: 'private-value', GH_TOKEN: 'git-secret' },
    fetchImpl, runTestCommand: async (_program, _args, { env }) => {
      tests += 1;
      assert.equal(env.ROSTER_API_KEY, undefined);
      assert.equal(env.GH_TOKEN, undefined);
      return { stdout: 'all tests pass', stderr: '' };
    },
  });
  assert.equal(result.mode, 'llm');
  assert.equal(result.turns, 2);
  assert.equal(tests, 2);
  assert.deepEqual(result.usage, { prompt_tokens: 46, completion_tokens: 17 });
  assert.match(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), /## Status\nReady/);
  assert.match(readFileSync(result.resultPath, 'utf8'), /node --test exited 0/);
  const memory = readFileSync(options.memoryPath, 'utf8');
  assert.match(memory, /"status":"llm"/);
  assert.ok(!memory.includes('private-value'));
});

test('budget exhaustion and a failed final test stop without claiming success', async (context) => {
  const budget = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
    .replace('model: ""', 'model: local-model').replace('turn_budget: 8', 'turn_budget: 1'));
  const options = fixture(context, budget);
  await assert.rejects(runCoder({
    ...options, fetchImpl: async () => ({ status: 200, json: async () => ({
      choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant',
        tool_calls: [{ id: 'call-1', type: 'function', function: {
          name: 'write_file', arguments: '{"path":"README.md","content":"changed"}',
        } }],
      } }],
    }) }),
  }), /turn budget .* exhausted/);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Example\n');
  assert.equal(JSON.parse(readFileSync(options.memoryPath, 'utf8')).status, 'failed');

  const failing = fixture(context, llmConfig);
  await assert.rejects(runCoder({
    ...failing, fetchImpl: async () => ({ status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done' } }],
    }) }),
    runTestCommand: async () => { throw Object.assign(new Error('failed'), {
      code: 1, stdout: 'not ok', stderr: 'one test failed',
    }); },
  }), /Final node --test failed [\s\S]*one test failed/);
  assert.equal(JSON.parse(readFileSync(failing.memoryPath, 'utf8')).status, 'failed');
});
