import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { writeAsk } from '../src/lib/ask.mjs';
import { runBuiltinIssue, stageReviewedFiles } from '../src/lib/builtin.mjs';
import { loadLearning } from '../src/lib/learn.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';
import { parseRecipe } from '../src/lib/recipe.mjs';
import { packAgentRun } from '../vendor/github-agent-contracts/scripts/parse-agent-run.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const stubConfig = parseConfig(example);
const llmConfig = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
  .replace('model: ""', 'model: local-model'));

function git(cwd, ...args) {
  return execFileSync('git', args, { cwd, encoding: 'utf8' }).trim();
}

function fixture(context) {
  const base = mkdtempSync(path.join(tmpdir(), 'roster-builtin-'));
  context.after(() => rmSync(base, { recursive: true, force: true }));
  const repoRoot = path.join(base, 'roster');
  const target = path.join(base, 'project');
  const contracts = path.join(base, 'contracts');
  mkdirSync(repoRoot);
  mkdirSync(target);
  mkdirSync(path.join(repoRoot, 'skills', 'implement-task'), { recursive: true });
  mkdirSync(path.join(contracts, 'scripts'), { recursive: true });
  writeFileSync(path.join(repoRoot, 'roster.config.example.yml'), example);
  writeFileSync(path.join(repoRoot, 'skills', 'implement-task', 'SKILL.md'), '# Code and test\n');
  writeFileSync(path.join(contracts, 'scripts', 'agent-pr.mjs'), 'export {};\n');
  writeFileSync(path.join(target, '.gitignore'), '.env\n.worktrees/\n');
  writeFileSync(path.join(target, 'AGENTS.md'), '# Agent instructions\nStay in the worktree.\n');
  writeFileSync(path.join(target, 'README.md'), '# Example\n');
  writeFileSync(path.join(target, 'smoke.test.mjs'),
    "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    "test('smoke', () => assert.equal(1, 1));\n");
  git(target, 'init', '-b', 'main');
  git(target, 'add', '--all');
  git(target, '-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '-m', 'Fixture setup');
  git(target, 'remote', 'add', 'origin', 'https://github.com/example/project.git');
  const cwd = path.join(target, 'nested');
  mkdirSync(cwd);
  const env = { ...process.env, GITHUB_AGENT_CONTRACTS: contracts };
  const issue = {
    number: 42, title: 'Add Status to README',
    body: 'Add a Status section to README.md.\n\n## Acceptance checks\n' +
      '- node --test exits 0\n- README has a Status section\n\n## Files allowed\n- `README.md`\n',
    url: 'https://github.com/example/project/issues/42',
  };
  const calls = [];
  const runCommand = async (program, args, workingDirectory) => {
    calls.push({ program, args, workingDirectory });
    if (program === 'gh') return JSON.stringify(issue);
    return git(workingDirectory, ...args);
  };
  return { base, repoRoot, target, cwd, env, contracts, issue, calls, runCommand };
}

test('roster ask writes a local draft ask, recipe, and executable task without network', async (context) => {
  const { repoRoot } = fixture(context);
  const result = await writeAsk('Add a Status section to README.md.', {
    repoRoot, config: stubConfig, id: 'draft-1',
    fetchImpl: () => { throw new Error('stub must not contact an LLM'); },
  });
  assert.equal(result.askPath, path.join(repoRoot, '.roster', 'asks', 'draft-1.md'));
  assert.equal(readFileSync(result.askPath, 'utf8'),
    '# Ask\n\nAdd a Status section to README.md.\n');
  assert.equal(parseRecipe(readFileSync(result.recipePath, 'utf8')).ask, 'local:draft-1');
  assert.match(readFileSync(result.taskPath, 'utf8'), /Files allowed\n- `README\.md`/);
  await assert.rejects(writeAsk('Another ask', { repoRoot, config: stubConfig, id: 'draft-1' }), /EEXIST/);
  await assert.rejects(writeAsk('', { repoRoot, config: stubConfig, id: 'draft-2' }), /Ask must be nonempty/);
});

test('builtin run reads the GitHub issue, creates a coder worktree, and stops at stub RESULT', async (context) => {
  const options = fixture(context);
  const logs = [];
  const result = await runBuiltinIssue(42, {
    ...options, config: stubConfig, log: (line) => logs.push(line),
    fetchImpl: () => { throw new Error('stub must not contact an LLM'); },
  });
  assert.equal(result.worktreePath, path.join(options.target, '.worktrees', 'issue-42'));
  assert.equal(git(result.worktreePath, 'branch', '--show-current'), 'issue-42');
  assert.match(readFileSync(result.assignmentPath, 'utf8'), /Issue URL: https:\/\/github.com\/example\/project\/issues\/42/);
  assert.match(readFileSync(result.assignmentPath, 'utf8'), /README has a Status section/);
  const recipe = parseRecipe(readFileSync(result.recipePath, 'utf8'));
  assert.equal(recipe.ask, 'issue:42');
  assert.deepEqual(recipe.seats.map(({ id }) => id), ['planner', 'coder']);
  assert.match(readFileSync(result.taskPath, 'utf8'), /## Acceptance checks/);
  assert.equal(readFileSync(result.taskPath, 'utf8'), result.planner.task);
  assert.deepEqual(result.sessions, { planner: 'roster-42-planner', coder: 'roster-42-coder' });
  assert.equal(readFileSync(result.envPath, 'utf8'),
    'AI_TASK=issue-42\nAI_SESSION=roster-42-coder\n');
  assert.match(readFileSync(result.result.resultPath, 'utf8'), /Deterministic stub only/);
  assert.match(result.result.summary, /Add Status to README/);
  assert.equal(result.run, result.runs.coder);
  assert.equal(result.result.mode, 'stub');
  assert.ok(logs[0].includes('node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "feat: issue 42"'));
  assert.equal((logs[0].match(/AI-Run:/g) ?? []).length, 2);
  assert.equal(result.runs.planner.line, packAgentRun(result.runs.planner.env));
  assert.equal(result.runs.coder.line, packAgentRun(result.runs.coder.env));
  assert.match(result.runs.planner.line, /\|roster-42-planner\|issue-42$/);
  assert.match(result.runs.coder.line, /\|roster-42-coder\|issue-42$/);
  assert.deepEqual(options.calls.map(({ program }) => program), ['git', 'git', 'gh', 'git']);
  assert.equal(options.calls.filter(({ program, args }) =>
    program === 'git' && args[0] === 'worktree').length, 1);
  assert.equal(JSON.parse(readFileSync(path.join(options.repoRoot,
    '.roster', 'memory', 'coder.jsonl'), 'utf8')).session, 'roster-42-coder');
  await assert.rejects(stageReviewedFiles(result.worktreePath, ['README.md']),
    /No reviewed task files changed/);
});

test('--publish requires an LLM and App environment before any GitHub or worktree action', async (context) => {
  const options = fixture(context);
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: stubConfig, publish: true,
    env: { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
  }), /requires an LLM endpoint/);
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, publish: true,
    env: { ...options.env, GITHUB_APP_ID: undefined, GITHUB_APP_PRIVATE_KEY_PATH: undefined },
  }), /GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH/);
  assert.deepEqual(options.calls, []);
});

test('LLM run stages only allowed code, supplies AI-Run fields, and invokes the SDK only with --publish', async (context) => {
  const options = fixture(context);
  const logs = [];
  let completion = 0;
  let published = 0;
  const fetchImpl = async (_url, request) => {
    completion += 1;
    const body = JSON.parse(request.body);
    assert.equal(body.model, 'local-model');
    assert.equal(request.headers.Authorization, 'Bearer private-key');
    assert.ok(!request.body.includes('private-key'));
    if (completion === 1) {
      assert.equal(body.tools, undefined);
      return { ok: true, json: async () => ({
        choices: [{ message: { content: JSON.stringify({
          title: 'Add Status to README',
          acceptance_checks: ['node --test exits 0', 'README has a Status section'],
          files_allowed: ['README.md'],
        }) } }],
        usage: { prompt_tokens: 5, completion_tokens: 2 },
      }) };
    }
    if (completion === 2) {
      assert.match(body.messages[1].content, /TASK\.md:\n# Task: Add Status to README/);
      assert.deepEqual(body.tools.map(({ function: tool }) => tool.name),
        ['read_file', 'write_file', 'list_dir', 'run_test']);
      return { ok: true, json: async () => ({
        choices: [{ finish_reason: 'tool_calls', message: {
          tool_calls: [{ id: 'update', type: 'function', function: {
            name: 'write_file',
            arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
          } }],
        } }],
        usage: { prompt_tokens: 10, completion_tokens: 3 },
      }) };
    }
    return { ok: true, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { content: 'Updated README; tests pass.' } }],
      usage: { prompt_tokens: 7, completion_tokens: 4 },
    }) };
  };
  const result = await runBuiltinIssue(42, {
    ...options, config: llmConfig,
    env: { ...options.env, ROSTER_API_KEY: 'private-key', GITHUB_APP_ID: '123',
      GITHUB_APP_PRIVATE_KEY_PATH: path.join(options.base, 'app.pem') },
    publish: true, log: (line) => logs.push(line), fetchImpl,
    publisher: async (program, args, publication) => {
      published += 1;
      assert.equal(program, process.execPath);
      assert.deepEqual(args, [
        path.join(options.contracts, 'scripts', 'agent-pr.mjs'),
        '--message', 'feat: issue 42',
      ]);
      assert.equal(publication.cwd, path.join(options.target, '.worktrees', 'issue-42'));
      assert.equal(publication.env.ROSTER_API_KEY, undefined);
      assert.equal(publication.env.GITHUB_APP_ID, '123');
      assert.equal(publication.env.AI_MODEL, 'local-model');
      assert.equal(publication.env.AI_CONTEXT_USED, '17');
      assert.equal(publication.env.AI_CONTEXT_OUT, '7');
      assert.equal(publication.env.AI_SESSION, 'roster-42-coder');
      assert.equal(git(publication.cwd, 'diff', '--cached', '--name-only'), 'README.md');
      return { stdout: 'Published via test SDK\n' };
    },
  });
  assert.equal(published, 1);
  assert.equal(completion, 3);
  assert.match(readFileSync(path.join(result.worktreePath, 'README.md'), 'utf8'), /## Status\nReady/);
  assert.equal(result.result.tests.exit_code, 0);
  assert.equal(result.run.line, packAgentRun(result.run.env));
  assert.match(result.runs.planner.line, /\|5\/-\|2\|roster-42-planner\|issue-42$/);
  assert.match(result.runs.coder.line, /\|17\/-\|7\|roster-42-coder\|issue-42$/);
  assert.equal((logs[0].match(/AI-Run:/g) ?? []).length, 2);
  assert.ok(logs.some((line) => line.includes('Published via test SDK')));
  assert.ok(!logs.join('\n').includes('private-key'));
});

test('opt-in learning records exactly one run for each builtin seat', async (context) => {
  const options = fixture(context);
  mkdirSync(path.join(options.target, '.roster', 'runs'), { recursive: true });
  const result = await runBuiltinIssue(42, {
    ...options, config: stubConfig, log: () => {},
    env: { ...options.env, GITHUB_AGENT_CONTRACTS: resolveContractsPath() },
    fetchImpl: () => { throw new Error('stub must not contact an LLM'); },
  });
  assert.deepEqual(loadLearning({ cwd: options.target }).runs, [
    { session: result.sessions.planner, task: 'issue-42', model: 'builtin-stub' },
    { session: result.sessions.coder, task: 'issue-42', model: 'builtin-stub' },
  ]);
});

test('detects a changed recipe after the coder runs tests and refuses publication', async (context) => {
  const options = fixture(context);
  let published = false;
  const fetchImpl = async (_url, request) => {
    const body = JSON.parse(request.body);
    if (!body.tools) {
      return { ok: true, json: async () => ({ choices: [{ message: { content: JSON.stringify({
        title: 'Update README', acceptance_checks: ['node --test exits 0'],
        files_allowed: ['README.md'],
      }) } }] }) };
    }
    return { ok: true, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { content: 'Done' } }],
    }) };
  };
  await assert.rejects(runBuiltinIssue(42, {
    ...options, config: llmConfig, publish: true, fetchImpl,
    env: { ...options.env, GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'app.pem' },
    runTestCommand: async (_program, _args, { cwd }) => {
      writeFileSync(path.join(cwd, 'RECIPE.yml'), 'tampered');
      return { stdout: 'passed', stderr: '' };
    },
    publisher: async () => { published = true; },
  }), /RECIPE\.yml changed after planning/);
  assert.equal(published, false);
  assert.equal(existsSync(path.join(options.target, '.worktrees', 'issue-42', 'RESULT.md')), true);
});

test('staging refuses changes outside the task scope', async (context) => {
  const options = fixture(context);
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  git(options.target, 'worktree', 'add', '-b', 'issue-42', worktree);
  writeFileSync(path.join(worktree, 'README.md'), '# Out of scope change\n');
  await assert.rejects(stageReviewedFiles(worktree, ['src/**']), /outside TASK.md scope/);
  assert.equal(git(worktree, 'diff', '--cached', '--name-only'), '');
});
