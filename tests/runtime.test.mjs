import assert from 'node:assert/strict';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { appendMemory, readMemory, seatMemoryPath } from '../src/runtime/memory.mjs';
import { loadSkills } from '../src/runtime/skills.mjs';
import { runCoder as runCoderSeat } from '../src/seats/coder.mjs';
import { runLoop } from '../src/runtime/loop.mjs';
import { withResearchSummary } from './helpers/research.mjs';

function runCoder(options) {
  return runCoderSeat({ ...options, fetchImpl: withResearchSummary(options.fetchImpl) });
}

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
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'),
    readFileSync(new URL('../principals/coder.md', import.meta.url), 'utf8'));
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(skillDirectory, 'SKILL.md'), '# Implement task\nRun tests.\n');
  writeFileSync(path.join(repoRoot, 'skills', 'run-tests', 'SKILL.md'), '# Run tests\nUse node --test.\n');
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nCode carefully.\n');
  writeFileSync(path.join(worktree, 'TASK.md'),
    planStub('Update `README.md` with a Status section and keep `smoke.test.mjs` in scope.',
      { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task);
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  writeFileSync(path.join(worktree, 'smoke.test.mjs'), '// Test fixture scope.\n');
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
  assert.deepEqual(await loadSkills({ repoRoot: options.repoRoot,
    task: '---\nskills: [implement-task, run-tests]\n---\n' }), [
    { name: 'implement-task', content: '# Implement task\nRun tests.\n' },
    { name: 'run-tests', content: '# Run tests\nUse node --test.\n' },
  ]);
  writeFileSync(options.memoryPath, '{bad-json\n');
  await assert.rejects(readMemory({ file: options.memoryPath, repoRoot: options.repoRoot }), /Invalid memory JSONL line 1/);
});

test('unrequested skills are optional, but requested malformed skill files fail', async (context) => {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-skills-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  assert.deepEqual(await loadSkills({ repoRoot }), []);
  const skills = path.join(repoRoot, 'skills');
  mkdirSync(skills);
  assert.deepEqual(await loadSkills({ repoRoot }), []);
  mkdirSync(path.join(skills, 'notes'));
  assert.deepEqual(await loadSkills({ repoRoot }), []);
  mkdirSync(path.join(skills, 'malformed'));
  mkdirSync(path.join(skills, 'malformed', 'SKILL.md'));
  assert.deepEqual(await loadSkills({ repoRoot }), []);
  await assert.rejects(loadSkills({ repoRoot, task: '---\nskills: [malformed]\n---\n' }), /regular SKILL\.md/);
});

test('a missing task skill stops the coder before any model or tool turn', async (context) => {
  const options = fixture(context, llmConfig);
  const taskPath = path.join(options.worktree, 'TASK.md');
  writeFileSync(taskPath, readFileSync(taskPath, 'utf8').replace(/^skills:.*$/m, 'skills: [missing]'));
  let calls = 0;
  await assert.rejects(runCoder({
    ...options,
    fetchImpl: () => { calls += 1; throw new Error('Unexpected model call'); },
    runTestCommand: () => { calls += 1; throw new Error('Unexpected test call'); },
  }), /Unknown task skill: missing/);
  assert.equal(calls, 0);
  assert.match(readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8'),
    /Checks: FAIL[\s\S]*Unknown task skill: missing/);
});

test('seat memory paths preserve a custom coder file and isolate the planner beside it', () => {
  const repoRoot = path.resolve('example-roster');
  assert.equal(seatMemoryPath({ repoRoot, memoryPath: 'custom/history.jsonl', seat: 'coder' }),
    path.join(repoRoot, 'custom', 'history.jsonl'));
  assert.equal(seatMemoryPath({ repoRoot, memoryPath: 'custom/history.jsonl', seat: 'planner' }),
    path.join(repoRoot, 'custom', 'planner.jsonl'));
  assert.throws(() => seatMemoryPath({ repoRoot, memoryPath: 'custom/history.jsonl', seat: 'merger' }),
    /planner or coder/);
  assert.throws(() => seatMemoryPath({ repoRoot, memoryPath: '.env.jsonl', seat: 'coder' }),
    /protected or secret path/);
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
  assert.equal(result.excellence.pass, false);
  assert.match(readFileSync(result.resultPath, 'utf8'), /Checks: FAIL/);
  assert.deepEqual(JSON.parse((await readMemory({
    file: options.memoryPath, repoRoot: options.repoRoot,
  }))[0]).status, 'stub');
});

test('disabling run_test prevents required-test tasks from contacting the model or running tests', async (context) => {
  const config = { ...llmConfig, tools: { ...llmConfig.tools, run_test: false } };
  const options = fixture(context, config);
  await assert.rejects(runCoder({
    ...options, env: {},
    fetchImpl: () => assert.fail('Required-test task must stop before a model request'),
    runTestCommand: () => assert.fail('Denied tests must not execute'),
  }), /run_test is disabled by tools\.run_test/);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Example\n');
  assert.match(readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8'), /Checks: FAIL/);
});

test('an explicit task waiver runs without offering or automatically invoking disabled tests', async (context) => {
  const config = { ...llmConfig, tools: { internet: true, run_test: false } };
  const options = fixture(context, config);
  const taskPath = path.join(options.worktree, 'TASK.md');
  writeFileSync(taskPath, readFileSync(taskPath, 'utf8').replace('---\n', '---\ntests: none\n'));
  const result = await runCoder({
    ...options, env: {},
    fetchImpl: async (_url, request) => {
      assert.deepEqual(JSON.parse(request.body).tools.map(({ function: tool }) => tool.name),
        ['read_file', 'write_file', 'list_dir', 'search_text']);
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Inspected README.' } }],
      }) };
    },
    runTestCommand: () => assert.fail('Disabled tests cannot be run automatically'),
  });
  assert.equal(result.testsSkipped, true);
  assert.equal(result.tests, undefined);
  assert.equal(result.excellence.pass, true);
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
    assert.match(sent.messages[0].content, /Principal coder:[\s\S]*No merge, no deploy/);
    assert.equal(sent.messages[0].content,
      readFileSync(path.join(options.worktree, 'CONTEXT.md'), 'utf8'));
    assert.ok(sent.messages[0].content.length <= options.config.seat.context_chars);
    assert.deepEqual(sent.tools.map((tool) => tool.function.name),
      ['read_file', 'write_file', 'list_dir', 'run_test', 'search_text']);
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
        content: 'Updated README Status; node --test passed.\nMODEL_ONLY_SOURCE_BODY',
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
  assert.deepEqual(result.response, { model: options.config.llm.model,
    usage: { prompt_tokens: 26, completion_tokens: 9 } });
  assert.equal(result.run.metrics.prompt_tokens, 26);
  assert.equal(result.run.metrics.completion_tokens, 9);
  assert.match(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), /## Status\nReady/);
  assert.match(readFileSync(result.resultPath, 'utf8'), /node --test exited 0/);
  const memory = readFileSync(options.memoryPath, 'utf8');
  assert.match(memory, /"status":"llm"/);
  assert.ok(!memory.includes('private-value'));
  assert.doesNotMatch(memory, /MODEL_ONLY_SOURCE_BODY/);
  const record = JSON.parse(memory);
  assert.ok(Number.isFinite(Date.parse(record.time)));
  assert.equal(record.issue, 4);
  assert.equal(record.task, 'issue-4');
  assert.match(record.changed, /README\.md/);
  assert.equal(record.tests, 'node --test exited 0');
  assert.equal(record.next_gap, 'None reported.');
});

test('named vLLM profile performs one worktree tool call then stops on a passing excellence gate', async (context) => {
  const config = parseConfig(example.replace('profile: ""', 'profile: vllm-local')
    .replace('model: ""', 'model: served-model'));
  const options = fixture(context, config);
  let turns = 0;
  let tests = 0;
  const result = await runCoder({
    ...options, env: { AI_PROVIDER: 'github-copilot' },
    fetchImpl: async (url, request) => {
      turns += 1;
      assert.equal(String(url), 'http://127.0.0.1:8000/v1/chat/completions');
      assert.equal(request.method, 'POST');
      const sent = JSON.parse(request.body);
      assert.equal(sent.model, 'served-model');
      assert.deepEqual(sent.tools.map(({ function: tool }) => tool.name),
        ['read_file', 'write_file', 'list_dir', 'run_test', 'search_text']);
      assert.match(sent.messages[0].content, /Principal coder:[\s\S]*## TASK\.md[\s\S]*Task skills/);
      if (turns === 1) return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({
              path: 'README.md', content: '# Example\n\n## Status\nReady.\n',
            }),
          } }],
        } }],
        usage: { prompt_tokens: 7, completion_tokens: 2 },
      }) };
      assert.equal(turns, 2, 'The coder must stop after verified completion');
      assert.deepEqual(JSON.parse(sent.messages.at(-1).content), { path: 'README.md', bytes: 28 });
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: 'Added a Status section; tests pass.',
        } }],
        usage: { prompt_tokens: 5, completion_tokens: 3 },
      }) };
    },
    runTestCommand: async () => {
      tests += 1;
      return { stdout: 'pass', stderr: '' };
    },
  });
  assert.equal(turns, 2);
  assert.equal(tests, 1);
  assert.equal(result.excellence.pass, true);
  assert.equal(result.run.provider, 'vllm');
  assert.equal(result.run.env.AI_PROVIDER, 'local');
  assert.equal(result.run.env.AI_MODEL, 'served-model');
  assert.equal(result.run.env.AI_CONTEXT_USED, '5');
  assert.equal(result.run.env.AI_CONTEXT_OUT, '3');
  assert.match(result.run.line, /^1\|local\|served-model@-\|/);
});

test('the final coder response replaces earlier usage instead of retaining totals or Copilot settings', async (context) => {
  for (const usage of [{ prompt_tokens: 100, completion_tokens: 40 }, undefined]) {
    const options = fixture(context, llmConfig);
    let turns = 0;
    const result = await runCoder({
      ...options, env: { AI_MODEL: 'GPT-6.1-Sol', AI_PROVIDER: 'github-copilot',
        AI_CONTEXT_USED: '1000000', AI_CONTEXT_MAX: '1000000', AI_CONTEXT_OUT: '999',
        AI_MODEL_VERSION: 'stale', ROSTER_API_KEY: 'test-only-key' },
      fetchImpl: async () => {
        turns += 1;
        if (turns === 1) return Response.json({
          choices: [{ finish_reason: 'tool_calls', message: {
            role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
              name: 'write_file', arguments: JSON.stringify({
                path: 'README.md', content: '# Example\n\n## Status\nReady.\n',
              }),
            } }],
          } }],
          model: 'earlier-response-model', usage: { prompt_tokens: 7, completion_tokens: 2 },
        });
        return Response.json({
          choices: [{ finish_reason: 'stop', message: {
            role: 'assistant', content: 'Added Status; tests passed.',
          } }],
          model: 'actual-final-model', ...(usage ? { usage } : {}),
        });
      },
      runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
    });
    assert.equal(turns, 2);
    assert.equal(result.excellence.pass, true);
    assert.equal(result.run.metrics.model, 'actual-final-model');
    assert.equal(result.run.metrics.provider, 'local');
    assert.equal(Object.hasOwn(result.run.metrics, 'context_max'), false);
    assert.equal(result.run.env.AI_CONTEXT_MAX, undefined);
    assert.equal(result.run.env.AI_MODEL_VERSION, '-');
    assert.match(readFileSync(result.resultPath, 'utf8'), /Model: actual-final-model/);
    if (usage) {
      assert.deepEqual(result.usage, { prompt_tokens: 107, completion_tokens: 42 });
      assert.equal(result.run.metrics.prompt_tokens, 100);
      assert.equal(result.run.metrics.completion_tokens, 40);
    } else {
      assert.deepEqual(result.usage, {});
      for (const field of ['prompt_tokens', 'completion_tokens', 'context_used', 'context_out']) {
        assert.equal(Object.hasOwn(result.run.metrics, field), false);
      }
      assert.equal(result.run.env.AI_CONTEXT_USED, undefined);
      assert.equal(result.run.env.AI_CONTEXT_OUT, undefined);
      assert.equal(result.response.usage, null);
    }
  }
});

test('garbage coder arguments get one repair then the scoped README Status fallback and final excellence', async (context) => {
  const options = fixture(context, llmConfig);
  writeFileSync(path.join(options.worktree, 'TASK.md'),
    planStub('Update `README.md` with a Status section.',
      { reference: 'issue:4', metadata: { task_class: 'feat', difficulty: 4 } }).task);
  let turns = 0;
  let tests = 0;
  const events = [];
  const result = await runCoder({
    ...options, env: {}, onEvent: async (event) => events.push(event),
    fetchImpl: async (_url, request) => {
      turns += 1;
      if (turns === 2) assert.match(JSON.parse(request.body).messages.at(-1).content, /Emit only tool_calls/);
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: `bad-${turns}`, type: 'function',
          function: { name: 'write_file', arguments: 'garbage' } }],
      } }], usage: { prompt_tokens: 100, completion_tokens: 40 } });
    },
    runTestCommand: async () => { tests += 1; return { stdout: 'pass', stderr: '' }; },
  });
  assert.equal(turns, 2);
  assert.equal(tests, 1);
  assert.equal(result.excellence.pass, true);
  assert.deepEqual(result.excellence.files, ['README.md']);
  assert.equal(result.implementationPath, 'deterministic-readme');
  assert.match(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), /## Status\nExperimental - APIs may change\./);
  assert.match(readFileSync(result.resultPath, 'utf8'), /Implementation path: deterministic-readme[\s\S]*## Files changed\n\n- README.md/);
  assert.ok(events.some((event) => event.type === 'implementation' && event.path === 'deterministic-readme'));
});

test('model-requested path escape is terminal, not a repairable failed test', async (context) => {
  const options = fixture(context, llmConfig);
  const outside = path.join(options.repoRoot, 'escape.md');
  let turns = 0;
  await assert.rejects(runCoder({
    ...options, env: {},
    fetchImpl: async (_url, request) => {
      turns += 1;
      if (turns < 3) {
        return { status: 200, json: async () => ({
          choices: [{ finish_reason: 'tool_calls', message: {
            role: 'assistant', tool_calls: [{ id: `edit-${turns}`, type: 'function', function: {
              name: 'write_file', arguments: JSON.stringify({
                path: turns === 1 ? '../escape.md' : 'README.md',
                content: '# Example\n\n## Status\nReady.\n',
              }),
            } }],
          } }],
        }) };
      }
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Updated README.' } }],
      }) };
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  }), /outside the worktree/);
  assert.equal(turns, 1);
  assert.equal(existsSync(outside), false);
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Example\n');
  assert.match(readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8'), /Checks: FAIL/);
});

test('failed final tests receive another turn before acceptance while usage and errors stay truthful', async (context) => {
  const options = fixture(context, llmConfig);
  let turns = 0;
  let tests = 0;
  const result = await runCoder({
    ...options, env: { ROSTER_API_KEY: 'private-value' },
    fetchImpl: async (_url, request) => {
      turns += 1;
      const sent = JSON.parse(request.body);
      if (turns === 2) {
        const feedback = sent.messages.at(-1);
        assert.equal(feedback.role, 'user');
        assert.match(feedback.content, /Final node --test failed \(exit 1\)/);
        assert.match(feedback.content, /not ok \[redacted\]/);
        assert.match(feedback.content, /test output also captured/);
        assert.match(feedback.content, /\[redacted\]/);
        assert.doesNotMatch(feedback.content, /private-value/);
        assert.match(feedback.content, /No change is verified yet/);
        return { status: 200, json: async () => ({
          choices: [{ finish_reason: 'tool_calls', message: {
            role: 'assistant', tool_calls: [{ id: 'repair', type: 'function', function: {
              name: 'write_file', arguments: JSON.stringify({
                path: 'README.md', content: '# Example\n\n## Status\nReady.\n',
              }),
            } }],
          } }],
          usage: { prompt_tokens: 1, completion_tokens: 1 },
        }) };
      }
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: turns === 1 ? 'Premature summary.' : 'Fixed the tests.',
        } }],
        usage: { prompt_tokens: 1, completion_tokens: 1 },
      }) };
    },
    runTestCommand: async () => {
      tests += 1;
      if (tests === 1) throw Object.assign(new Error('tests failed'), {
        code: 1, stdout: 'not ok private-value', stderr: 'warning: test output also captured',
      });
      return { stdout: 'pass', stderr: '' };
    },
  });
  assert.equal(turns, 3);
  assert.equal(tests, 2);
  assert.equal(result.excellence.pass, true);
  assert.deepEqual(result.usage, { prompt_tokens: 3, completion_tokens: 3 });
  assert.match(readFileSync(result.resultPath, 'utf8'), /Checks: PASS/);
});

test('first exit 1 gets a repair turn and excellence waits for green tests', async () => {
        let turns = 0;
        let tests = 0;
        let verifies = 0;
        const events = [];
        const task = planStub('Update README.md and smoke.test.mjs.').task;
        const result = await runLoop({ config: llmConfig, context: { task, pack: task },
          tools: {
            write_file: async () => ({ path: 'README.md', bytes: 1 }),
            run_test: async () => ({ exit_code: ++tests === 1 ? 1 : 0, stdout: 'failing assertion', stderr: '' }),
          },
          onEvent: (event) => events.push(event), env: {},
          fetchImpl: async (_url, request) => {
            turns += 1;
            if (turns === 2) {
              assert.equal(verifies, 0);
              assert.match(JSON.parse(request.body).messages.at(-1).content, /Repair 1 of 4[\s\S]*Rerun node --test/);
              return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
                role: 'assistant', tool_calls: [{ id: 'repair', type: 'function', function: {
                  name: 'write_file', arguments: '{"path":"README.md","content":"fixed"}',
                } }],
              } }] });
            }
            return Response.json({ choices: [{ finish_reason: 'stop', message: {
              role: 'assistant', content: 'Done.',
            } }] });
          },
          verify: (candidate) => {
            verifies += 1;
            assert.equal(candidate.tests.exit_code, 0);
            return { pass: true, reasons: [] };
          },
        });
        assert.equal(result.error, undefined);
        assert.equal(result.testRepairs, 1);
        assert.equal(tests, 2);
        assert.equal(verifies, 1);
        assert.deepEqual(events.filter(({ type }) => type === 'test-repair'),
          [{ type: 'test-repair', attempt: 1, budget: 4 }]);
});

test('a failed test defers the rest of its tool batch until a repair turn reads the summary', async () => {
          let turns = 0;
          let writes = 0;
          let tests = 0;
          const task = planStub('Update README.md and smoke.test.mjs.').task;
          const result = await runLoop({ config: llmConfig, context: { task, pack: task }, env: {},
            tools: {
              write_file: async () => { writes += 1; return { path: 'README.md', bytes: 1 }; },
              run_test: async () => ({ exit_code: ++tests === 1 ? 1 : 0, stdout: 'not ok', stderr: '' }),
            },
            fetchImpl: async (_url, request) => {
              turns += 1;
              const call = (id, name, args) => ({ id, type: 'function',
                function: { name, arguments: JSON.stringify(args) } });
              if (turns === 2) {
                assert.equal(writes, 0);
                const messages = JSON.parse(request.body).messages;
                assert.match(messages.at(-2).content, /Deferred after failed tests/);
                assert.match(messages.at(-1).content, /Repair 1 of 4/);
              }
              const calls = turns === 1 ? [call('test', 'run_test', {}), call('deferred', 'write_file',
                { path: 'README.md', content: 'premature' })]
                : turns === 2 ? [call('repair', 'write_file', { path: 'README.md', content: 'fixed' })] : null;
              return Response.json({ choices: [{ finish_reason: calls ? 'tool_calls' : 'stop', message: calls
                ? { role: 'assistant', tool_calls: calls } : { role: 'assistant', content: 'Fixed.' } }] });
            },
            verify: () => ({ pass: true, reasons: [] }),
          });
          assert.equal(result.error, undefined);
          assert.equal(result.testRepairs, 1);
          assert.equal(writes, 1);
        });

test('an optional test that exits 1 cannot be waived into a successful final summary', async () => {
          let tests = 0;
          let turns = 0;
          let verifies = 0;
          const task = planStub('Update README.md and smoke.test.mjs.').task
            .replace('---\n', '---\ntests: none\n');
          const result = await runLoop({ config: llmConfig, context: { task, pack: task }, env: {},
            tools: { run_test: async () => ({ exit_code: 1, stdout: `failure ${++tests}`, stderr: '' }) },
            fetchImpl: async () => {
              turns += 1;
              return Response.json({ choices: [{ finish_reason: turns === 1 ? 'tool_calls' : 'stop', message: turns === 1
                ? { role: 'assistant', tool_calls: [{ id: 'test', type: 'function',
                  function: { name: 'run_test', arguments: '{}' } }] }
                : { role: 'assistant', content: 'Done.' } }] });
            },
            verify: () => { verifies += 1; return { pass: true, reasons: [] }; },
          });
          assert.match(result.error.message, /Test repair budget \(4\) exhausted/);
          assert.equal(result.testRepairs, 4);
          assert.equal(tests, 5);
          assert.equal(verifies, 0);
});

test('repair turns can reach their tool allowance without ending on the first failed check', async () => {
            let turns = 0;
            let tests = 0;
            let changed = false;
            const task = planStub('Update README.md and smoke.test.mjs.').task;
            const result = await runLoop({ config: { ...llmConfig, seat: { ...llmConfig.seat, turn_budget: 1 } },
              context: { task, pack: task }, env: {},
              tools: {
                write_file: async () => { changed = true; return { path: 'README.md', bytes: 1 }; },
                run_test: async () => { tests += 1; return { exit_code: changed ? 0 : 1, stdout: 'check', stderr: '' }; },
              },
              fetchImpl: async (_url, request) => {
                turns += 1;
                if (turns === 3) assert.match(JSON.parse(request.body).messages.at(-1).content, /Repair tests are green/);
                return Response.json({ choices: [{ finish_reason: turns === 2 ? 'tool_calls' : 'stop', message: turns === 2
                  ? { role: 'assistant', tool_calls: [{ id: 'repair', type: 'function',
                    function: { name: 'write_file', arguments: '{"path":"README.md","content":"fixed"}' } }] }
                  : { role: 'assistant', content: 'Done.' } }] });
              },
              verify: (candidate) => {
                assert.equal(candidate.tests.exit_code, 0);
                return { pass: true, reasons: [] };
              },
            });
            assert.equal(result.error, undefined);
            assert.equal(result.testRepairs, 1);
            assert.equal(result.turns, 3);
            assert.equal(tests, 3);
});

test('an unsafe final-test side effect stops the loop without asking the model to hide it', async (context) => {
  const options = fixture(context, llmConfig);
  let turns = 0;
  await assert.rejects(runCoder({
    ...options, env: {},
    fetchImpl: async () => {
      turns += 1;
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }],
      }) };
    },
    runTestCommand: async () => {
      writeFileSync(path.join(options.worktree, '.env'), 'SECRET=do-not-publish\n');
      return { stdout: 'pass', stderr: '' };
    },
  }), /Diff path is protected or outside TASK\.md allowed paths: \.env/);
  assert.equal(turns, 1);
  assert.match(readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8'), /Checks: FAIL/);
});

test('a nonzero run_test returns captured output for the coder to fix in the next turn', async (context) => {
  const options = fixture(context, llmConfig);
  let turns = 0;
  let testRuns = 0;
  const fetchImpl = async (_url, request) => {
    turns += 1;
    const sent = JSON.parse(request.body);
    if (turns === 1) {
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', content: null,
          tool_calls: [{ id: 'failed-test', type: 'function',
            function: { name: 'run_test', arguments: '{}' } }],
        } }],
      }) };
    }
    if (turns === 2) {
      assert.match(sent.messages.at(-1).content, /Repair 1 of 4/);
      assert.deepEqual(JSON.parse(sent.messages.at(-2).content), {
        exit_code: 1, stdout: 'not ok', stderr: 'assertion failed',
      });
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', content: null,
          tool_calls: [{ id: 'fix-code', type: 'function', function: {
            name: 'write_file',
            arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
          } }],
        } }],
      }) };
    }
    assert.match(sent.messages.at(-1).content, /README\.md/);
    return { status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Fixed tests.' } }],
    }) };
  };
  const result = await runCoder({
    ...options, env: { ROSTER_API_KEY: 'test-only-key' }, fetchImpl,
    runTestCommand: async (_program, _args, { timeout }) => {
      testRuns += 1;
      assert.equal(timeout, 300_000);
      if (testRuns === 1) throw Object.assign(new Error('tests failed'), {
        code: 1, stdout: 'not ok', stderr: 'assertion failed',
      });
      return { stdout: 'all tests pass', stderr: '' };
    },
  });
  assert.equal(result.mode, 'llm');
  assert.equal(result.turns, 3);
  assert.equal(testRuns, 2);
  assert.equal(result.tests.exit_code, 0);
  assert.match(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), /## Status\nReady/);
  assert.equal(JSON.parse(readFileSync(options.memoryPath, 'utf8')).status, 'llm');
});

test('budget exhaustion and a failed final test stop without claiming success', async (context) => {
  const budget = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
    .replace('model: ""', 'model: local-model').replace('turn_budget: 8', 'turn_budget: 1'));
  const options = fixture(context, budget);
  await assert.rejects(runCoder({
    ...options, vault: { get: async () => undefined },
    fetchImpl: async () => ({ status: 200, json: async () => ({
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
  assert.match(readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8'), /First failure: Coder turn budget/);

  const failing = fixture(context, llmConfig);
  let finalTurns = 0;
  let finalTests = 0;
  await assert.rejects(runCoder({
    ...failing, vault: { get: async () => undefined },
    fetchImpl: async () => {
      finalTurns += 1;
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done' } }],
      }) };
    },
    runTestCommand: async () => {
      finalTests += 1;
      throw Object.assign(new Error('failed'), {
        code: 1, stdout: 'not ok', stderr: 'one test failed',
      });
    },
  }), /Final node --test failed [\s\S]*one test failed/);
  assert.equal(finalTurns, 5);
  assert.equal(finalTests, 5);
  assert.equal(JSON.parse(readFileSync(failing.memoryPath, 'utf8')).status, 'failed');
  assert.match(readFileSync(path.join(failing.worktree, 'RESULT.md'), 'utf8'),
    /Checks: FAIL[\s\S]*one test failed[\s\S]*Test repair budget \(4\) exhausted/);
});

test('coder stops exploring and asks for a write before the turn budget is spent', async () => {
  const task = planStub('Update README.md.').task;
  let reads = 0;
  let turns = 0;
  const result = await runLoop({
    config: { ...llmConfig, seat: { ...llmConfig.seat, turn_budget: 8 } },
    context: { task, pack: task }, env: {},
    tools: {
      read_file: async () => { reads += 1; return 'file'; },
      run_test: async () => ({ exit_code: 0, stdout: 'pass', stderr: '' }),
    },
    fetchImpl: async (_url, request) => {
      turns += 1;
      const body = JSON.parse(request.body);
      if (turns === 4) assert.match(body.messages.at(-1).content, /Exploration budget used/);
      if (turns === 5) {
        assert.match(body.messages.at(-1).content, /exploration budget used/i);
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Blocked before an edit.' } }] });
      }
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: `read-${turns}`, type: 'function', function: {
          name: 'read_file', arguments: JSON.stringify({ path: 'README.md' }),
        } }],
      } }] });
    },
    verify: () => ({ pass: true, reasons: [] }),
  });
  assert.equal(reads, 3);
  assert.equal(turns, 5);
  assert.match(result.summary, /Blocked before an edit/);
  assert.equal(result.error, undefined);
});
