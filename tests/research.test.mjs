import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub, taskFilesAllowed } from '../src/planner/stub.mjs';
import { runResearch } from '../src/runtime/research.mjs';
import { createTools } from '../src/runtime/tools.mjs';
import { runCoder } from '../src/seats/coder.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model'));

async function fixture(context, allowed = 'src/**') {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-research-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(path.join(worktree, 'src'), { recursive: true });
  const task = planStub(`Inspect the requested change.\n\n## Files allowed\n- \`${allowed}\`\n`,
    { title: 'Implement the task', metadata: { task_class: 'feat', difficulty: 4 } }).task;
  writeFileSync(path.join(worktree, 'TASK.md'), task);
  const tools = await createTools({ worktree, allowedFiles: taskFilesAllowed(task) });
  return { repoRoot, worktree, tools, expectedTask: task, env: {}, vault: { get: async () => undefined } };
}

test('stub research reads no more than eight files and two hundred lines without write_file or source edits', async (context) => {
  const options = await fixture(context);
  const contents = Array.from({ length: 201 }, (_, index) => `source-line-${index + 1}`).join('\n');
  for (let index = 0; index < 10; index += 1) {
    writeFileSync(path.join(options.worktree, 'src', `file-${index}.mjs`), contents);
  }
  const reads = [];
  let writes = 0;
  const result = await runResearch({
    ...options,
    tools: { ...options.tools,
      read_file: async (args) => { reads.push(args); return await options.tools.read_file(args); },
      write_file: () => { writes += 1; assert.fail('Research must not use write_file'); } },
    fetchImpl: () => assert.fail('Stub research must not contact an LLM'),
  });
  assert.equal(writes, 0);
  assert.equal(result.turns, 0);
  assert.equal(result.files.length, 8);
  assert.equal(result.truncated, true);
  assert.ok(result.files.every(({ lines }) => lines === 200));
  assert.equal(reads.filter(({ path: file }) => file !== 'TASK.md').length, 8);
  assert.ok(reads.filter(({ path: file }) => file !== 'TASK.md').every(({ max_lines }) => max_lines === 200));
  const report = readFileSync(result.researchPath, 'utf8');
  assert.match(report, /## What the task asks/);
  assert.match(report, /## What exists/);
  assert.match(report, /## Gaps/);
  assert.match(report, /source-line-200/);
  assert.doesNotMatch(report, /source-line-201|file-8\.mjs|file-9\.mjs/);
  for (let index = 0; index < 10; index += 1) {
    assert.equal(readFileSync(path.join(options.worktree, 'src', `file-${index}.mjs`), 'utf8'), contents);
  }
  assert.deepEqual(readdirSync(options.worktree).sort(), ['RESEARCH.md', 'TASK.md', 'src']);
  await assert.rejects(options.tools.write_file({ path: 'RESEARCH.md', content: 'changed' }), /not allowed/);
  await assert.rejects(runResearch(options), /EEXIST/);
});

test('a missing allowed file is an explicit gap and a changed task stops research', async (context) => {
  const options = await fixture(context, 'src/new.mjs');
  const result = await runResearch(options);
  assert.equal(result.files[0].status, 'missing');
  assert.match(readFileSync(result.researchPath, 'utf8'), /Allowed file not yet present: src\/new\.mjs/);
  writeFileSync(path.join(options.worktree, 'TASK.md'), options.expectedTask.replace('Implement', 'Changed'));
  await assert.rejects(runResearch(options), /TASK\.md changed/);
});

test('an optional failed or tool-requesting model summary leaves the inventory intact', async (context) => {
  for (const malformed of [false, true]) {
    const options = await fixture(context, 'src/new.mjs');
    let calls = 0;
    const result = await runResearch({ ...options, config,
      fetchImpl: async (_url, request) => {
        calls += 1;
        const body = JSON.parse(request.body);
        assert.equal(body.tools, undefined);
        assert.ok(body.messages[1].content.length <= 8000);
        assert.match(readFileSync(path.join(options.worktree, 'RESEARCH.md'), 'utf8'), /src\/new\.mjs/);
        if (!malformed) throw new Error('RAW_ENDPOINT_DIAGNOSTIC_MUST_NOT_BE_STORED');
        return { status: 200, json: async () => ({ choices: [{
          finish_reason: 'tool_calls', message: { role: 'assistant', content: null,
            tool_calls: [{ id: 'bad', type: 'function',
              function: { name: 'write_file', arguments: '{}' } }] },
        }] }) };
      } });
    assert.equal(calls, 1);
    assert.equal(result.turns, 1);
    assert.equal(result.summaryStatus, 'failed');
    const report = readFileSync(result.researchPath, 'utf8');
    assert.match(report, /src\/new\.mjs/);
    assert.match(report, /Optional LLM research summary failed/);
    assert.doesNotMatch(report, /RAW_ENDPOINT_DIAGNOSTIC_MUST_NOT_BE_STORED/);
    assert.deepEqual(readdirSync(path.join(options.worktree, 'src')), []);
  }
});

test('a locked fleet model substitution in research is a route failure, not an optional summary failure', async (context) => {
  const options = await fixture(context, 'src/new.mjs');
  const substituted = async () => Response.json({ model: 'substituted-model', choices: [{
    finish_reason: 'stop', message: { role: 'assistant', content: 'Inventory reviewed.' } }] });
  const locked = { ...config, llm: { ...config.llm, locked_model: 'local-model' } };
  await assert.rejects(runResearch({ ...options, config: locked, fetchImpl: substituted }),
    (error) => error.code === 'ROSTER_LOCKED_MODEL_MISMATCH');
  rmSync(path.join(options.worktree, 'RESEARCH.md'));
  const result = await runResearch({ ...options, config, fetchImpl: substituted });
  assert.equal(result.summaryStatus, 'complete');
});

test('coder research precedes writes and its usage is included in coder accounting', async (context) => {
  const options = await fixture(context, 'README.md');
  cpSync(new URL('../principals/', import.meta.url), path.join(options.repoRoot, 'principals'), { recursive: true });
  cpSync(new URL('../skills/', import.meta.url), path.join(options.repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(options.worktree, 'AGENTS.md'), '# Instructions\nStay scoped.\n');
  writeFileSync(path.join(options.worktree, 'README.md'), '# Before\n');
  let calls = 0;
  const result = await runCoder({ ...options, config, task: 'issue-8', session: 'coder-8',
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.model, 'local-model');
      if (calls === 1) {
        assert.equal(body.tools, undefined);
        assert.match(body.messages[0].content, /builtin research step/);
        assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# Before\n');
        return { status: 200, json: async () => ({
          choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'README exists; edit and test remain.' } }],
          usage: { prompt_tokens: 2, completion_tokens: 1 },
        }) };
      }
      assert.match(readFileSync(path.join(options.worktree, 'RESEARCH.md'), 'utf8'), /README exists/);
      if (calls === 2) return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', content: null,
          tool_calls: [{ id: 'edit', type: 'function', function: { name: 'write_file',
            arguments: JSON.stringify({ path: 'README.md', content: '# After\n' }) } }] } }],
        usage: { prompt_tokens: 5, completion_tokens: 2 },
      }) };
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Updated README; tests passed.' } }],
        usage: { prompt_tokens: 3, completion_tokens: 1 },
      }) };
    }, runTestCommand: async () => ({ stdout: 'tests pass', stderr: '' }) });
  assert.equal(calls, 3);
  assert.equal(result.turns, 2);
  assert.equal(result.research.turns, 1);
  assert.equal(result.research.summaryStatus, 'complete');
  assert.deepEqual(result.usage, { prompt_tokens: 10, completion_tokens: 4 });
  assert.equal(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), '# After\n');
});
