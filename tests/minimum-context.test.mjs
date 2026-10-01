import assert from 'node:assert/strict';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { taskContextPolicy } from '../src/runtime/context-policy.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { runReviewer } from '../src/seats/reviewer.mjs';

const sourceRoot = fileURLToPath(new URL('../', import.meta.url));
const config = parseConfig(readFileSync(join(sourceRoot, 'roster.config.example.yml'), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: docs-model'));

function fixture(t, metadata = { task_class: 'docs', difficulty: 1 }) {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-minimum-context-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = join(repoRoot, 'worktree');
  mkdirSync(worktree);
  for (const name of ['read-before-write', 'small-diff']) {
    cpSync(join(sourceRoot, 'skills', name), join(repoRoot, 'skills', name), { recursive: true });
  }
  const taskText = planStub('Add a one-line Status section to README.md.', { metadata }).task;
  writeFileSync(join(worktree, 'TASK.md'), taskText);
  writeFileSync(join(worktree, 'README.md'), '# Project\n');
  return { repoRoot, worktree, taskText, task: 'issue-92', session: 'roster-92-coder', config, env: {} };
}

test('difficulty1 docs uses exactly TASK, allowed files, read-before-write and small-diff without auxiliary loads', async (t) => {
  const options = fixture(t);
  writeFileSync(join(options.worktree, 'AGENTS.md'), 'UNNEEDED_AGENTS_MARKER');
  writeFileSync(join(options.worktree, 'RESEARCH.md'), 'UNNEEDED_RESEARCH_MARKER');
  mkdirSync(join(options.repoRoot, '.roster', 'memory'), { recursive: true });
  writeFileSync(join(options.repoRoot, '.roster', 'memory', 'coder.jsonl'), 'invalid unused memory');
  const context = await loadContext({ ...options, memoryPath: join(options.repoRoot, '.roster', 'memory', 'coder.jsonl'),
    priorFeedback: 'UNNEEDED_FEEDBACK_MARKER' });
  assert.equal(context.minimalDocs, true);
  assert.deepEqual(context.skillNames, ['read-before-write', 'small-diff']);
  assert.deepEqual(context.skills.map(({ name }) => name), context.skillNames);
  assert.deepEqual(context.memory, []);
  assert.equal(context.agents, null);
  assert.match(context.pack, /## Issue Ask\n\nAdd a one-line Status section to README.md/);
  assert.match(context.pack, /## TASK.md\n\n# Outcome:/);
  assert.deepEqual(context.skills.map(({ name }) => name), ['read-before-write', 'small-diff']);
  assert.doesNotMatch(context.pack, /skills: \[|difficulty:|estimate_min:/);
  assert.doesNotMatch(context.pack, /UNNEEDED_|## Principal|## Seat memory|## Task skills/);
  assert.equal(existsSync(join(options.repoRoot, 'principals')), false);
});

test('minimum docs coder makes no research request and still checks tests, file scope and secrets', async (t) => {
  const options = fixture(t);
  let calls = 0;
  let tests = 0;
  const result = await runCoder({
    ...options,
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.doesNotMatch(body.messages[0].content, /builtin research step|## Principal/);
      assert.deepEqual(body.tools.map(({ function: tool }) => tool.name),
        ['read_file', 'write_file', 'list_dir', 'run_test', 'search_text']);
      return Response.json({ choices: [calls === 1 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Project\n\n## Status\nActive.\n' }),
        } }],
      } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Added Status; tests passed.' } }],
      usage: { prompt_tokens: 100, completion_tokens: 40 } });
    },
    runTestCommand: async () => { tests += 1; return { stdout: 'pass', stderr: '' }; },
  });
  assert.equal(calls, 2);
  assert.equal(tests, 1);
  assert.equal(result.excellence.pass, true);
  assert.deepEqual(result.excellence.files, ['README.md']);
  assert.deepEqual(result.stages, ['context', 'skills', 'tool_loop', 'memory', 'excellence', 'result']);
  assert.equal(result.research, undefined);
  assert.equal(result.researchPath, undefined);
  assert.equal(existsSync(join(options.worktree, 'RESEARCH.md')), false);
  assert.equal(result.run.metrics.prompt_tokens, 100);
});

test('minimum docs keeps path deny checks even when the model requests a protected write', async (t) => {
  const options = fixture(t);
  let calls = 0;
  const result = await runCoder({
    ...options, fetchImpl: async (_url, request) => {
      calls += 1;
      if (calls === 2) assert.match(JSON.parse(request.body).messages.at(-1).content, /not allowed/);
      return Response.json({ choices: [calls < 3 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: `edit-${calls}`, type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: calls === 1 ? '.github/workflows/ci.yml' : 'README.md',
            content: '# Project\n\n## Status\nActive.\n' }),
        } }],
      } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Updated only README.' } }] });
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.equal(result.excellence.pass, true);
  assert.equal(existsSync(join(options.worktree, '.github')), false);
  assert.deepEqual(result.excellence.files, ['README.md']);
});

test('minimum docs still fails on required tests and does not hide secrets in the actual edit', async (t) => {
  for (const failure of ['tests', 'secret']) {
    const options = fixture(t);
    let calls = 0;
    await assert.rejects(runCoder({
      ...options, config: { ...config, seat: { ...config.seat, turn_budget: 2 } },
      env: { ROSTER_API_KEY: 'test-only-sensitive-key' },
      fetchImpl: async () => {
        calls += 1;
        return Response.json({ choices: [calls === 1 ? { finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'README.md',
              content: failure === 'secret' ? '# Project\ntest-only-sensitive-key\n' : '# Project\n\n## Status\nActive.\n' }),
          } }],
        } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }] });
      },
      runTestCommand: async () => {
        if (failure === 'tests') throw Object.assign(new Error('failed'), { code: 1, stdout: 'fail', stderr: '' });
        return { stdout: 'pass', stderr: '' };
      },
    }), failure === 'tests' ? /node --test failed|turn budget|Final node/ : /Secret material/);
  }
});

test('only difficulty4+ feat may load the normal research and implementation path', async (t) => {
  for (const metadata of [{ task_class: 'feat', difficulty: 4 }, { task_class: 'feat', difficulty: 5 }]) {
    const options = fixture(t, metadata);
    assert.equal(taskContextPolicy(options.taskText).minimum, false);
    await assert.rejects(loadContext({ ...options }), /principal|ENOENT/);
  }
});

test('minimum docs reviewer stays read-only without loading a principal file', async (t) => {
  const options = fixture(t);
  const resultPath = join(options.worktree, 'RESULT.md');
  writeFileSync(resultPath, '# Result\nChecks: FAIL\n');
  const review = await runReviewer({ ...options, coderResult: { resultPath, mode: 'stub', excellence: { pass: false, files: [] } },
    fetchImpl: () => assert.fail('A failed coder is not sent for model review') });
  assert.equal(review.verdict, 'fail');
  assert.equal(existsSync(join(options.repoRoot, 'principals')), false);
  assert.match(readFileSync(review.reviewPath, 'utf8'), /no passing implementation/);
});
