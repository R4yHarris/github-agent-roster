import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { passingReview } from './helpers/review.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { loadContext } from '../src/runtime/context.mjs';
import { taskContextPolicy } from '../src/runtime/context-policy.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { runReviewer } from '../src/seats/reviewer.mjs';
import { createTools } from '../src/runtime/tools.mjs';

const sourceRoot = fileURLToPath(new URL('../', import.meta.url));
const config = parseConfig(readFileSync(join(sourceRoot, 'roster.config.example.yml'), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: deepseek-v4.1-flash'));

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

test('truncated tool calls are not executed and the complete tool_calls retry is accepted', async (t) => {
  const options = fixture(t);
  let calls = 0;
  const caps = [];
  const result = await runCoder({ ...options, fetchImpl: async (_url, request) => {
    calls += 1;
    caps.push(JSON.parse(request.body).max_tokens);
    if (calls === 2) {
      assert.equal(readFileSync(join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
      assert.ok(!request.body.includes('PRIVATE_TRUNCATED_BODY'));
    }
    if (calls < 3) return Response.json({ choices: [{ finish_reason: calls === 1 ? 'length' : 'tool_calls',
      message: { role: 'assistant', tool_calls: [{ id: 'readme', type: 'function', function: {
        name: 'write_file', arguments: JSON.stringify({ path: 'README.md',
          content: calls === 1 ? 'PRIVATE_TRUNCATED_BODY' : '# Project\n\n## Status\nActive.\n' }),
      } }] } }] });
    return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Added Status.' } }] });
  }, runTestCommand: async () => ({ stdout: 'pass', stderr: '' }) });
  assert.equal(result.excellence.pass, true);
  assert.equal(result.turns, 3);
  assert.deepEqual(caps, [8192, 8192, 8192]);
  assert.doesNotMatch(readFileSync(join(options.worktree, 'README.md'), 'utf8'), /PRIVATE_TRUNCATED_BODY/);
});

test('a README write survives one docs length continue without reasoning at 8192', async (t) => {
  const options = fixture(t);
  const caps = [];
  let calls = 0;
  const result = await runCoder({ ...options, fetchImpl: async (_url, request) => {
    calls += 1;
    const body = JSON.parse(request.body);
    caps.push(body.max_tokens);
    assert.equal(body.reasoning_effort, 'none');
    assert.equal(body.chat_template_kwargs.thinking, false);
    if (calls === 1) return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
      role: 'assistant', tool_calls: [{ id: 'readme', type: 'function', function: {
        name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Project\n\n## Status\nActive.\n' }),
      } }],
    } }] });
    if (calls === 3) {
      assert.equal(body.messages.at(-2).content, 'PRIVATE_TRUNCATED_RESPONSE');
      assert.match(body.messages.at(-1).content, /Continue it from where it stopped/);
    }
    return Response.json({ choices: [{ finish_reason: calls === 2 ? 'length' : 'stop', message: {
      role: 'assistant', content: calls === 2 ? 'PRIVATE_TRUNCATED_RESPONSE' : 'Added Status.',
    } }] });
  }, runTestCommand: async () => ({ stdout: 'pass', stderr: '' }) });
  assert.equal(result.excellence.pass, true);
  assert.equal(result.turns, 3);
  assert.deepEqual(caps, [8192, 8192, 8192]);
  assert.match(readFileSync(join(options.worktree, 'README.md'), 'utf8'), /## Status/);
});

test('a second docs length passes review when the required Status section is already saved', async (t) => {
  const options = fixture(t);
  execFileSync('git', ['init'], { cwd: options.worktree, stdio: 'ignore' });
  execFileSync('git', ['add', 'README.md', 'TASK.md'], { cwd: options.worktree, stdio: 'ignore' });
  execFileSync('git', ['-c', 'user.name=Roster Test', '-c', 'user.email=roster@example.invalid',
    'commit', '-m', 'fixture'], { cwd: options.worktree, stdio: 'ignore' });
  let calls = 0;
  let tests = 0;
  const result = await runCoder({ ...options, fetchImpl: async (_url, request) => {
    calls += 1;
    const body = JSON.parse(request.body);
    if (calls === 1) return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
      role: 'assistant', tool_calls: [{ id: 'readme', type: 'function', function: {
        name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Project\n\n## Status\nActive.\n' }),
      } }],
    } }] });
    assert.equal(body.max_tokens, 8192);
    assert.equal(body.reasoning_effort, 'none');
    assert.equal(body.chat_template_kwargs.thinking, false);
    return Response.json({ choices: [{ finish_reason: 'length',
      message: { role: 'assistant', content: 'PRIVATE_BODY' } }] });
  }, runTestCommand: async () => { tests += 1; return { stdout: 'pass', stderr: '' }; } });
  assert.equal(calls, 3);
  assert.equal(tests, 0);
  assert.equal(result.excellence.pass, true);
  assert.equal(result.finishReason, undefined);
  assert.match(readFileSync(join(options.worktree, 'README.md'), 'utf8'), /## Status/);
  const review = await runReviewer({ ...options, coderResult: result,
    fetchImpl: async (_url, request) => Response.json({ choices: [{ finish_reason: 'stop', message: {
      role: 'assistant', content: passingReview(JSON.parse(request.body), {
        reasons: ['The saved Status section and passing checks satisfy the task.'],
        security_notes: ['Only README.md changed.'] }),
    } }] }) });
  assert.equal(review.verdict, 'pass', review.content);
  assert.doesNotMatch(readFileSync(result.resultPath, 'utf8') + review.content, /PRIVATE_BODY/);
});

test('a README-only docs slice offers no list_dir and a directory walk fails the seat', async (t) => {
  const options = fixture(t);
  let calls = 0;
  await assert.rejects(runCoder({ ...options, fetchImpl: async (_url, request) => {
    calls += 1;
    const body = JSON.parse(request.body);
    const offered = body.tools.map(({ function: tool }) => tool.name);
    assert.ok(!offered.includes('list_dir'), offered.join(','));
    assert.ok(!offered.includes('search_text'), offered.join(','));
    assert.equal(body.max_tokens, 8192);
    assert.equal(body.chat_template_kwargs.thinking, false);
    return Response.json({ choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant',
      tool_calls: [{ id: 'walk', type: 'function', function: {
        name: 'list_dir', arguments: JSON.stringify({ path: '.' }) } }] } }] });
  }, runTestCommand: () => assert.fail('A refused directory walk must not run tests') }),
  /invalid or unavailable tool/);
  assert.equal(calls, 1);
  assert.equal(readFileSync(join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
});

test('docs tools exclude list_dir and one write received during test returns to draft', async (t) => {
  const options = fixture(t);
  let calls = 0;
  let tests = 0;
  const result = await runCoder({
    ...options,
    askKind: 'slice',
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.ok(!(body.tools ?? []).some(({ function: tool }) => tool.name === 'list_dir'));
      if (calls === 1) {
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'first-save', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({
              path: 'README.md', content: '# Project\n\n## Status\nDraft.\n',
            }),
          } }],
        } }] });
      }
      if (calls === 2) {
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'late-save', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({
              path: 'README.md', content: '# Project\n\n## Status\nActive.\n',
            }),
          } }],
        } }] });
      }
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: 'Saved the final Status and reran its check.',
      } }] });
    },
    runTestCommand: async () => {
      tests += 1;
      return { stdout: 'pass', stderr: '' };
    },
  });
  assert.equal(result.excellence.pass, true);
  assert.equal(calls, 3);
  assert.equal(tests, 0);
  assert.equal(result.testsSkipped, true);
  assert.match(readFileSync(join(options.worktree, 'README.md'), 'utf8'), /## Status\nActive\./);
});

test('a bounded docs write skips node tests and proceeds to its summary', async (t) => {
  const options = fixture(t);
  let calls = 0;
  let tests = 0;
  const result = await runCoder({
    ...options,
    askKind: 'slice',
    fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [calls === 1 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'save', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({
            path: 'README.md', content: '# Project\n\n## Status\nActive.\n',
          }),
        } }],
      } } : { finish_reason: 'stop', message: {
        role: 'assistant', content: 'Saved the Status section; docs-only tests were skipped.',
      } }] });
    },
    runTestCommand: async () => {
      tests += 1;
      throw new Error('Docs-only changes must not run node --test');
    },
  });
  assert.equal(calls, 2);
  assert.equal(tests, 0);
  assert.equal(result.testRepairs, 0);
  assert.equal(result.testsSkipped, true);
  assert.equal(result.excellence.pass, true);
});

test('a bounded docs slice naming one non-README file is not offered a directory walk', async (t) => {
  const options = fixture(t);
  const taskText = options.taskText.replaceAll('README.md', 'docs/guide.md');
  writeFileSync(join(options.worktree, 'TASK.md'), taskText);
  mkdirSync(join(options.worktree, 'docs'));
  writeFileSync(join(options.worktree, 'docs', 'guide.md'), '# Guide\n');
  let calls = 0;
  let offered;
  const result = await runCoder({
    ...options,
    fetchImpl: async (_url, request) => {
      calls += 1;
      const names = JSON.parse(request.body).tools.map(({ function: tool }) => tool.name);
      if (calls === 1) offered = names;
      return Response.json({ choices: [calls === 1 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'save-guide', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({
            path: 'docs/guide.md', content: '# Guide\n\n## Status\nActive.\n',
          }),
        } }],
      } } : { finish_reason: 'stop', message: {
        role: 'assistant', content: 'Saved the guide and passed its check.',
      } }] });
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.equal(result.excellence.pass, true);
  assert.ok(!offered.includes('list_dir'), offered.join(','));
});

test('an unknown finish reason after a README write fails review by name without printing its response', async (t) => {
  const options = fixture(t);
  let calls = 0;
  let tests = 0;
  let failed;
  await assert.rejects(runCoder({ ...options, fetchImpl: async () => {
    calls += 1;
    if (calls === 1) return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
      role: 'assistant', tool_calls: [{ id: 'readme', type: 'function', function: {
        name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Project\n\n## Status\nActive.\n' }),
      } }],
    } }] });
    return Response.json({ model: 'failed-response-model',
      usage: { prompt_tokens: 17, completion_tokens: 9 },
      choices: [{ finish_reason: 'eos_token', message: { role: 'assistant', content: 'PRIVATE_RESPONSE_BODY' } }] });
  }, runTestCommand: async () => { tests += 1; return { stdout: 'pass', stderr: '' }; } }), (error) => {
    failed = error.result;
    return /finish reason: eos_token/.test(error.message) && !error.message.includes('PRIVATE_RESPONSE_BODY');
  });
  assert.equal(calls, 2);
  assert.equal(tests, 0);
  assert.equal(failed.finishReason, 'eos_token');
  assert.equal(failed.run.env.AI_MODEL, 'failed-response-model');
  assert.equal(failed.run.env.AI_CONTEXT_USED, '17');
  assert.match(readFileSync(join(options.worktree, 'README.md'), 'utf8'), /## Status/);
  assert.doesNotMatch(readFileSync(failed.resultPath, 'utf8'), /PRIVATE_RESPONSE_BODY/);
  const review = await runReviewer({ ...options, coderResult: failed,
    fetchImpl: () => assert.fail('Failed finish reason must not request reviewer inference') });
  assert.equal(review.verdict, 'fail');
  assert.match(review.content, /Unsupported LLM finish reason: eos_token/);
  assert.doesNotMatch(review.content, /PRIVATE_RESPONSE_BODY/);
});

test('an uninitialized contracts worktree writes blocked result and review without test-repair inference', async (t) => {
  const options = fixture(t, { task_class: 'fix', difficulty: 2 });
  writeFileSync(join(options.worktree, 'TASK.md'),
    planStub('Update README.md and smoke.test.mjs.', {
      metadata: { task_class: 'fix', difficulty: 2 },
    }).task);
  writeFileSync(join(options.worktree, 'smoke.test.mjs'), '// Fixture test.\n');
  writeFileSync(join(options.worktree, '.gitmodules'),
    '[submodule "github-agent-contracts"]\n\tpath = vendor/github-agent-contracts\n\turl = fixture\n');
  const events = [];
  let blocked;
  await assert.rejects(runCoder({ ...options, askKind: 'slice', onEvent: (event) => events.push(event),
    fetchImpl: async () => Response.json({ choices: [{ finish_reason: 'stop',
      message: { role: 'assistant', content: 'Done.' } }] }),
    runTestCommand: () => assert.fail('Missing declared contracts must be detected before running node tests'),
  }), (error) => {
    blocked = error.result;
    return error.message === 'Contracts submodule was not initialized';
  });
  assert.equal(blocked.blocked, true);
  assert.equal(blocked.testRepairs, 0);
  const output = readFileSync(blocked.resultPath, 'utf8');
  assert.match(output, /Outcome: blocked \(contracts infrastructure\)[\s\S]*Checks: BLOCKED/);
  assert.doesNotMatch(output, /vendor\//);
  const review = await runReviewer({ ...options, askKind: 'slice', coderResult: blocked,
    fetchImpl: () => assert.fail('Infrastructure-blocked work must not request reviewer inference') });
  assert.equal(review.verdict, 'fail');
  assert.match(review.content, /infrastructure-blocked, not a slice test failure/);
  assert.ok(!events.some(({ type }) => type === 'test-repair'));
});

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

test('minimum docs coder makes no research request and skips tests while checking scope and secrets', async (t) => {
  const options = fixture(t);
  let calls = 0;
  let tests = 0;
  const result = await runCoder({
    ...options,
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.reasoning_effort, 'none');
      assert.equal(body.max_tokens, 8192);
      assert.doesNotMatch(body.messages[0].content, /builtin research step|## Principal/);
      assert.deepEqual(body.tools.map(({ function: tool }) => tool.name), calls === 1
        ? ['read_file', 'write_file', 'edit_file'] : []);
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
  assert.equal(tests, 0);
  assert.equal(result.testsSkipped, true);
  assert.equal(result.packBudgetChars, 200000);
  assert.equal(result.excellence.pass, true);
  assert.deepEqual(result.excellence.files, ['README.md']);
  assert.deepEqual(result.stages, ['context', 'skills', 'tool_loop', 'memory', 'excellence', 'result']);
  assert.equal(result.research, undefined);
  assert.equal(result.researchPath, undefined);
  assert.equal(existsSync(join(options.worktree, 'RESEARCH.md')), false);
  assert.equal(result.run.metrics.prompt_tokens, 100);
  assert.equal(result.run.metrics.effort, '-');
  assert.doesNotMatch(readFileSync(join(options.repoRoot, '.roster', 'memory', 'coder.jsonl'), 'utf8'), /reasoning_content/);
});

test('a sole README save skips node tests before refusing a later empty search', async (t) => {
  const options = fixture(t);
  mkdirSync(join(options.worktree, 'tests'), { recursive: true });
  writeFileSync(join(options.worktree, 'tests', 'repl.test.mjs'), '');
  const events = [];
  let calls = 0;
  let tests = 0;
  const result = await runCoder({
    ...options,
    askKind: 'slice',
    onEvent: (event) => events.push(event),
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      if (calls === 1) {
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'read', type: 'function', function: {
            name: 'read_file', arguments: JSON.stringify({ path: 'README.md' }),
          } }],
        } }] });
      }
      if (calls === 2) {
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [
            { id: 'save', type: 'function', function: {
              name: 'write_file', arguments: JSON.stringify({
                path: 'README.md', content: '# Project\n\n## Status\nActive.\n',
              }),
            } },
            { id: 'reread', type: 'function', function: {
              name: 'read_file', arguments: JSON.stringify({ path: 'README.md' }),
            } },
          ],
        } }] });
      }
      assert.equal(body.tools, undefined);
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'empty-search', type: 'function', function: {
          name: 'search_text', arguments: JSON.stringify({ query: '', path: 'README.md' }),
        } }],
      } }] });
    },
    runTestCommand: async () => {
      tests += 1;
      throw new Error('Docs-only changes must not run node --test');
    },
  });
  assert.equal(calls, 3);
  assert.equal(tests, 0);
  assert.equal(result.testsSkipped, true);
  assert.equal(result.excellence.pass, true);
  assert.equal(events.filter((event) => event.type === 'tool' && event.name === 'read_file').length, 1);
  assert.ok(!events.some((event) => event.type === 'tool' && event.name === 'search_text'));
  assert.ok(events.some((event) =>
    event.type === 'tool-result' && event.name === 'search_text' && event.status === 'denied'));
});

test('a docs slice saves its named file without offering run_test', async (t) => {
  const options = fixture(t);
  const events = [];
  let calls = 0;
  let tests = 0;
  const result = await runCoder({
    ...options,
    askKind: 'slice',
    onEvent: (event) => events.push(event),
    fetchImpl: async (_url, request) => {
      calls += 1;
      assert.ok(!JSON.parse(request.body).tools.some(({ function: tool }) => tool.name === 'run_test'));
      if (calls === 1) {
        return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [
            { id: 'save', type: 'function', function: {
              name: 'write_file', arguments: JSON.stringify({
                path: 'README.md', content: '# Project\n\n## Status\nActive.\n',
              }),
            } },
          ],
        } }] });
      }
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: 'Saved the named file and passed its check.',
      } }] });
    },
    runTestCommand: async () => {
      tests += 1;
      assert.match(readFileSync(join(options.worktree, 'README.md'), 'utf8'), /## Status/);
      return { stdout: 'pass', stderr: '' };
    },
  });
  assert.equal(result.excellence.pass, true);
  assert.equal(tests, 0);
  assert.ok(events.some((event) => event.type === 'tool' && event.name === 'write_file'));
  assert.ok(!events.some((event) => event.type === 'tool' && event.name === 'run_test'));
});

test('docs1 README-only tools deny planner fixtures, RESEARCH and repo search; reads may precede the required write', async (t) => {
  const options = fixture(t);
  mkdirSync(join(options.worktree, 'tests', 'fixtures'), { recursive: true });
  writeFileSync(join(options.worktree, 'tests', 'fixtures', 'planner-task-92.md'), 'PRIVATE_PLANNER_FIXTURE');
  writeFileSync(join(options.worktree, 'RESEARCH.md'), 'PRIVATE_RESEARCH');
  let tests = 0;
  const tools = await createTools({ worktree: options.worktree, allowedFiles: ['README.md'], readmeOnlyDocs: true,
    runCommand: async () => { tests += 1; return { stdout: 'pass', stderr: '' }; } });
  assert.equal(await tools.read_file({ path: 'TASK.md' }), options.taskText);
  assert.equal(await tools.read_file({ path: 'README.md' }), '# Project\n');
  for (const path of ['tests/fixtures/planner-task-92.md', 'RESEARCH.md', 'AGENTS.md', 'tests']) {
    await assert.rejects(tools.read_file({ path }), /may read only TASK\.md and README\.md/);
  }
  await assert.rejects(tools.search_text({ query: 'PRIVATE', path: '.' }), /does not allow repository search/);
  await assert.rejects(tools.list_dir({ path: 'tests' }), /does not allow directory listing/);
  await assert.rejects(tools.run_test(), /must write README\.md before running tests/);
  assert.equal(tests, 0);
  writeFileSync(join(options.worktree, 'README.md'), '# Project\n\n## Status\nActive.\n');
  assert.deepEqual(await tools.run_test(),
    { exit_code: 0, skipped: true, stdout: 'docs-only: tests skipped', stderr: '' });
  assert.equal(tests, 0);
  await tools.write_file({ path: 'README.md', content: '# Project\n\n## Status\nActive.\n' });
  assert.deepEqual(await tools.run_test(),
    { exit_code: 0, skipped: true, stdout: 'docs-only: tests skipped', stderr: '' });
  assert.equal(tests, 0);
  await assert.rejects(tools.read_file({ path: 'tests/fixtures/planner-task-92.md' }), /may read only/);
});

test('an existing Status section can finish with docs-only tests skipped', async (t) => {
  const options = fixture(t);
  writeFileSync(join(options.worktree, 'README.md'), '# Project\n\n## Status\nActive.\n');
  let calls = 0;
  let tests = 0;
  const result = await runCoder({
    ...options,
    fetchImpl: async () => {
      calls += 1;
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: 'README.md already has the required Status section and the checks passed.',
      } }] });
    },
    runTestCommand: async () => {
      tests += 1;
      return { stdout: 'pass\n', stderr: '' };
    },
  });
  assert.equal(calls, 1);
  assert.equal(tests, 0);
  assert.equal(result.testsSkipped, true);
  assert.equal(result.excellence.pass, true);
  const written = readFileSync(join(options.worktree, 'RESULT.md'), 'utf8');
  assert.match(written, /Tests skipped: docs-only/);
});

test('docs1 scope is exact; other task classes/difficulties/file sets keep their current tool behavior', async (t) => {
  const options = fixture(t);
  assert.equal(taskContextPolicy(options.taskText).readmeOnlyDocs, true);
  for (const metadata of [{ task_class: 'docs', difficulty: 2 }, { task_class: 'feat', difficulty: 1 }]) {
    assert.equal(taskContextPolicy(planStub('Update README.md.', { metadata }).task).readmeOnlyDocs, false);
  }
  assert.equal(taskContextPolicy(planStub('Update README.md and docs/guide.md.', {
    metadata: { task_class: 'docs', difficulty: 1 },
  }).task).readmeOnlyDocs, false);
  await assert.rejects(createTools({ worktree: options.worktree, allowedFiles: ['**/*'], readmeOnlyDocs: true }),
    /scope limited to README\.md/);
});

test('non-README slices send difficulty-based effort and keep denying fixture and harness reads until repeats stop the coder', async (t) => {
  for (const difficulty of [1, 5]) {
    const options = fixture(t, { task_class: 'fix', difficulty });
    mkdirSync(join(options.worktree, 'src', 'runtime'), { recursive: true });
    mkdirSync(join(options.worktree, 'tests', 'fixtures'), { recursive: true });
    writeFileSync(join(options.worktree, 'src', 'widget.mjs'), 'export const ready = false;\n');
    writeFileSync(join(options.worktree, 'src', 'runtime', 'loop.mjs'), 'PRIVATE_HARNESS');
    writeFileSync(join(options.worktree, 'tests', 'fixtures', 'planner.md'), 'PRIVATE_FIXTURE');
    writeFileSync(join(options.worktree, 'TASK.md'),
      planStub('Fix src/widget.mjs.', { metadata: { task_class: 'fix', difficulty } }).task);
    let calls = 0;
    await assert.rejects(runCoder({ ...options, askKind: 'slice', fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      assert.equal(body.reasoning_effort, difficulty === 1 ? 'low' : 'high');
      assert.ok(!request.body.includes('PRIVATE_FIXTURE') && !request.body.includes('PRIVATE_HARNESS'));
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: `call-${calls}`, type: 'function', function: {
          name: 'read_file', arguments: JSON.stringify({ path: 'tests/fixtures/planner.md' }),
        } }],
      } }] });
    }, runTestCommand: () => assert.fail('No relevant test file exists, so node --test must not run') }),
      /not allowed by TASK\.md slice scope \(repeated after 2 denials\)/);
    assert.equal(calls, 3);
    assert.equal(readFileSync(join(options.worktree, 'src', 'widget.mjs'), 'utf8'),
      'export const ready = false;\n');
  }
});

test('configured docs slice keeps denying planner fixtures, offers exact file tools, and skips tests', async (t) => {
  const options = fixture(t);
  mkdirSync(join(options.worktree, 'tests', 'fixtures'), { recursive: true });
  writeFileSync(join(options.worktree, 'tests', 'fixtures', 'planner-task-92.md'), 'PRIVATE_FIXTURE_92');
  let calls = 0;
  await assert.rejects(runCoder({ ...options, fetchImpl: async (_url, request) => {
    calls += 1;
    const body = JSON.parse(request.body);
    assert.deepEqual(body.tools.map(({ function: tool }) => tool.name), ['read_file', 'write_file', 'edit_file']);
    assert.deepEqual(body.tools[0].function.parameters.properties.path.enum, ['TASK.md', 'README.md']);
    assert.deepEqual(body.tools[1].function.parameters.properties.path.enum, ['README.md']);
    assert.deepEqual(body.tools[2].function.parameters.properties.path.enum, ['README.md']);
    assert.ok(!request.body.includes('PRIVATE_FIXTURE_92'));
    return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
      role: 'assistant', tool_calls: [{ id: `call-${calls}`, type: 'function', function: {
        name: 'read_file', arguments: JSON.stringify({ path: 'tests/fixtures/planner-task-92.md' }),
      } }],
    } }] });
  }, runTestCommand: () => assert.fail('Docs-only changes must not run node --test') }),
    /may read only TASK\.md and README\.md.*\(repeated after 2 denials\)/);
  assert.equal(calls, 3);
  assert.equal(readFileSync(join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
});

test('docs-only final completion cannot claim success without writing README, even if tests are waived', async (t) => {
  for (const waived of [false, true]) {
    const options = fixture(t);
    if (waived) writeFileSync(join(options.worktree, 'TASK.md'), options.taskText.replace('---\n', '---\ntests: none\n'));
    await assert.rejects(runCoder({ ...options, fetchImpl: async () => Response.json({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'All done; tests passed.' } }],
    }), runTestCommand: () => assert.fail('No tests may run before writing README') }), /must write README\.md/);
    const result = readFileSync(join(options.worktree, 'RESULT.md'), 'utf8');
    assert.match(result, /Checks: FAIL/);
    assert.doesNotMatch(result, /Checks: PASS|All done; tests passed/);
  }
});

test('coder HTTP timeout yields explicit unverified RESULT and failing incomplete review without reviewer inference', async (t) => {
  const options = fixture(t);
  const timeoutConfig = { ...config, llm: { ...config.llm, request_timeout_ms: 10 } };
  let failed;
  let calls = 0;
  await assert.rejects(runCoder({ ...options, config: timeoutConfig, fetchImpl: async () => {
    calls += 1;
    if (calls === 1) return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
      role: 'assistant', tool_calls: [{ id: 'read', type: 'function', function: {
        name: 'read_file', arguments: JSON.stringify({ path: 'README.md' }),
      } }],
    } }] });
    return new Promise(() => {});
  }, runTestCommand: () => assert.fail('A timed-out coder cannot run verification tests') }),
  (error) => {
    failed = error.result;
    return error.cause?.code === 'ROSTER_LLM_TIMEOUT';
  });
  assert.equal(failed.timedOut, true);
  assert.equal(failed.excellence.pass, false);
  const result = readFileSync(failed.resultPath, 'utf8');
  assert.match(result, /Outcome: timed out \(unverified\)[\s\S]*Checks: FAIL/);
  assert.match(result, /Unverified summary[\s\S]*Coder HTTP request timed out\. No change was verified/);
  assert.doesNotMatch(result, /Checks: PASS|Operational checks passed|## Summary\n/);
  assert.equal(readFileSync(join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
  const review = await runReviewer({ ...options,
    config: { ...config, llm: { ...config.llm, base_url: '' } }, coderResult: failed,
    fetchImpl: () => assert.fail('Timeout review must not ask a model for a verdict'),
  });
  assert.equal(review.verdict, 'fail');
  assert.equal(review.queried, false);
  assert.match(review.content, /Verdict: fail[\s\S]*HTTP timeout[\s\S]*review was not completed/);
  assert.doesNotMatch(review.content, /Verdict: pass|passed review/);
});

test('minimum docs keeps path deny checks even when the model requests a protected write', async (t) => {
  const options = fixture(t);
  let calls = 0;
  await assert.rejects(runCoder({
    ...options, fetchImpl: async (_url, request) => {
      calls += 1;
      return Response.json({ choices: [{ finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'edit-1', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: '.github/workflows/ci.yml',
            content: '# Project\n\n## Status\nActive.\n' }),
        } }],
      } }] });
    },
    runTestCommand: () => assert.fail('Docs-only changes must not run node --test'),
  }), /not allowed/);
  assert.equal(calls, 1);
  assert.equal(existsSync(join(options.worktree, '.github')), false);
  assert.equal(readFileSync(join(options.worktree, 'README.md'), 'utf8'), '# Project\n');
});

test('minimum docs skips node tests but still rejects secrets in the actual edit', async (t) => {
  const docs = fixture(t);
  let docsCalls = 0;
  const result = await runCoder({
    ...docs,
    fetchImpl: async () => {
      docsCalls += 1;
      return Response.json({ choices: [docsCalls === 1 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({
            path: 'README.md', content: '# Project\n\n## Status\nActive.\n',
          }),
        } }],
      } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }] });
    },
    runTestCommand: () => assert.fail('Docs-only changes must not run node --test'),
  });
  assert.equal(result.testsSkipped, true);
  assert.equal(result.excellence.pass, true);

  const secret = fixture(t);
  let secretCalls = 0;
  await assert.rejects(runCoder({
    ...secret,
    env: { ROSTER_API_KEY: 'test-only-sensitive-key' },
    fetchImpl: async () => {
      secretCalls += 1;
      return Response.json({ choices: [secretCalls === 1 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({
            path: 'README.md', content: '# Project\ntest-only-sensitive-key\n',
          }),
        } }],
      } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }] });
    },
    runTestCommand: () => assert.fail('Docs-only changes must not run node --test'),
  }), /Secret material/);
});

test('coder AI-Run records the served model and summed usage', async (t) => {
  const options = fixture(t);
  let calls = 0;
  const result = await runCoder({
    ...options,
    fetchImpl: async () => {
      calls += 1;
      return Response.json({ model: 'served-model', usage: { prompt_tokens: 10 * calls, completion_tokens: calls },
        choices: [calls === 1 ? { finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Project\n\n## Status\nActive.\n' }),
          } }],
        } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Updated README.' } }] });
    },
    runTestCommand: () => assert.fail('Docs-only changes must not run node --test'),
  });
  assert.equal(result.run.metrics.model, 'served-model');
  assert.equal(result.run.metrics.context_out, 2);
});

test('a failed final-summary turn after green checks keeps the run and omits empty tools', async (t) => {
  const options = fixture(t);
  let calls = 0;
  const result = await runCoder({
    ...options,
    fetchImpl: async (_url, request) => {
      calls += 1;
      if (calls === 2) {
        assert.equal(Object.hasOwn(JSON.parse(request.body), 'tools'), false);
        return Response.json({ error: { message: 'all backends failed' } }, { status: 502 });
      }
      return Response.json({ model: 'served-model', usage: { prompt_tokens: 10, completion_tokens: 1 },
        choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'edit', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Project\n\n## Status\nActive.\n' }),
          } }],
        } }] });
    },
    runTestCommand: () => assert.fail('Docs-only changes must not run node --test'),
  });
  assert.equal(calls, 2);
  assert.equal(result.excellence.pass, true);
  assert.equal(result.run.metrics.model, 'served-model');
});

test('credential-shaped fixture gets one secret correction, then passes with a sentinel', async (t) => {
  const options = fixture(t);
  const credentialShaped = ['sk', 'abcdefghijklmnopqrstuvwxyz0123'].join('-');
  let calls = 0;
  const result = await runCoder({
    ...options,
    fetchImpl: async (_url, request) => {
      calls += 1;
      const last = JSON.parse(request.body).messages.at(-1).content;
      if (calls === 3) {
        assert.match(last, /One secret-material correction is allowed/);
        assert.match(last, /test-only-private-api-key/);
        assert.doesNotMatch(last, new RegExp(credentialShaped));
      }
      const content = calls === 1 ? `# Project\n${credentialShaped}\n` : '# Project\ntest-only-private-api-key\n';
      return Response.json({ choices: [calls === 1 || calls === 3 ? { finish_reason: 'tool_calls', message: {
        role: 'assistant', tool_calls: [{ id: `edit-${calls}`, type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content }),
        } }],
      } } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Updated README.' } }] });
    },
    runTestCommand: () => assert.fail('Docs-only changes must not run node --test'),
  });
  assert.equal(calls, 4);
  assert.equal(result.excellence.pass, true);
  assert.match(readFileSync(join(options.worktree, 'README.md'), 'utf8'), /test-only-private-api-key/);
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
