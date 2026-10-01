import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { submitAsk } from '../src/lib/ask.mjs';
import { renderIssueBody } from '../src/lib/issue.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example);
const llmConfig = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
  .replace('model: ""', 'model: local-model'));

function fixture(t) {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-ask-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  return repoRoot;
}

test('available gh creates an issue in the current GitHub repository without a local draft', async (t) => {
  const repoRoot = fixture(t);
  const calls = [];
  const runCommand = async (program, args, cwd, env) => {
    calls.push({ program, args, cwd, promptDisabled: env.GH_PROMPT_DISABLED });
    if (program === 'gh' && args[0] === '--version') return 'gh version test';
    if (program === 'git') return 'https://github.com/example/project.git\n';
    return 'https://github.com/example/project/issues/42\n';
  };
  const ask = 'Add status to README.md.\n\n## Acceptance checks\n- Show status';
  const result = await submitAsk(ask, { repoRoot, cwd: repoRoot, config, env: {}, runCommand });
  assert.deepEqual(result, {
    mode: 'issue', url: 'https://github.com/example/project/issues/42',
    number: 42, title: 'Add status to README.md.', askKind: 'slice',
  });
  assert.deepEqual(calls.map(({ program, args }) => [program, args]), [
    ['gh', ['--version']],
    ['git', ['remote', 'get-url', 'origin']],
    ['gh', ['issue', 'create', '--repo', 'example/project',
      '--title', 'Add status to README.md.', '--body', renderIssueBody(ask)]],
  ]);
  assert.match(calls[2].args.at(-1),
    /^# Ask\n\nAdd status to README\.md\.[\s\S]*task_class: feat\ndifficulty: 2\nestimate_min: 15\n$/);
  assert.ok(calls.every(({ cwd, promptDisabled }) => cwd === repoRoot && promptDisabled === '1'));
  assert.equal(existsSync(join(repoRoot, '.roster', 'asks')), false);
});

test('missing gh writes a deterministic draft and prints a body-file create command', async (t) => {
  const repoRoot = fixture(t);
  const calls = [];
  const result = await submitAsk('Fix README.md.', {
    repoRoot, cwd: repoRoot, config: llmConfig, env: {}, id: 'draft-1',
    fetchImpl: () => assert.fail('Offline draft must not contact an LLM'),
    runCommand: async (program, args) => {
      calls.push([program, args]);
      throw Object.assign(new Error('gh not installed'), { code: 'ENOENT' });
    },
  });
  assert.equal(result.mode, 'draft');
  assert.equal(readFileSync(result.askPath, 'utf8'), renderIssueBody('Fix README.md.'));
  assert.match(readFileSync(result.recipePath, 'utf8'), /ask: local:draft-1/);
  assert.match(readFileSync(result.taskPath, 'utf8'), /Fix README\.md/);
  assert.match(result.command, /^gh issue create --title .+ --body-file .+$/);
  assert.ok(result.command.includes(result.askPath));
  assert.deepEqual(calls, [['gh', ['--version']]]);
});

test('conventional task class is in the issue body before gh creates the issue', async (t) => {
  const repoRoot = fixture(t);
  const calls = [];
  await submitAsk('fix(cli): Correct README.md.', {
    repoRoot, cwd: repoRoot, config, env: {},
    runCommand: async (program, args) => {
      calls.push([program, args]);
      if (program === 'gh' && args[0] === '--version') return 'gh version test';
      if (program === 'git') return 'https://github.com/example/project.git';
      return 'https://github.com/example/project/issues/43';
    },
  });
  assert.match(calls[2][1].at(-1),
    /task_class: fix\ndifficulty: 2\nestimate_min: 15\n$/);
  assert.equal(existsSync(join(repoRoot, '.roster', 'asks')), false);
});

test('offline feature and initiative asks write PLAN only, with child issue drafts rather than a new queue', async (t) => {
  const repoRoot = fixture(t);
  for (const [id, ask, askKind] of [['feature', 'Implement the profile feature.', 'feature'],
    ['initiative', 'build an orchestrator', 'initiative']]) {
    const result = await submitAsk(ask, {
      repoRoot, cwd: repoRoot, config: llmConfig, env: {}, id,
      fetchImpl: () => assert.fail('Offline planning cannot contact a model'),
      runCommand: async () => { throw Object.assign(new Error('Missing gh'), { code: 'ENOENT' }); },
    });
    assert.equal(result.mode, 'draft');
    assert.equal(result.askKind, askKind);
    assert.equal(result.taskPath, undefined);
    assert.equal(result.recipePath, undefined);
    assert.equal(existsSync(join(repoRoot, '.roster', 'asks', id, 'TASK.md')), false);
    const plan = readFileSync(result.planPath, 'utf8');
    assert.match(plan, /## Outcomes[\s\S]*## Waves[\s\S]*Labels: `wave:1`/);
    assert.match(result.command, /^gh issue create --title .+ --body-file .+$/);
  }
});

test('online broad asks create only the parent issue; clarify does not contact GitHub or infer scope', async (t) => {
  const repoRoot = fixture(t);
  const calls = [];
  const runCommand = async (program, args) => {
    calls.push([program, args]);
    if (program === 'git') return 'https://github.com/example/project.git';
    return args[0] === '--version' ? 'gh version test' : 'https://github.com/example/project/issues/42';
  };
  const planned = await submitAsk('build an orchestrator', { repoRoot, config, env: {}, runCommand });
  assert.equal(planned.askKind, 'initiative');
  assert.equal(planned.mode, 'issue');
  assert.equal(calls.filter(([program, args]) => program === 'gh' && args[0] === 'issue').length, 1);
  const clarify = await submitAsk('Improve things.', {
    repoRoot, config, env: {}, runCommand: () => assert.fail('Clarify must not contact GitHub'),
  });
  assert.equal(clarify.mode, 'clarify');
  assert.equal(clarify.askKind, 'clarify');
  assert.match(clarify.clarification, /one concrete outcome[\s\S]*allowed files/);
  assert.equal(existsSync(join(repoRoot, '.roster', 'asks')), false);
});

test('gh creation errors and unexpected URLs are not disguised as offline drafts', async (t) => {
  const repoRoot = fixture(t);
  const base = { repoRoot, cwd: repoRoot, config, env: {} };
  const runCommand = async (program, args) => {
    if (program === 'gh' && args[0] === '--version') return 'gh version test';
    if (program === 'git') return 'https://github.com/example/project.git\n';
    throw new Error('HTTP 403');
  };
  await assert.rejects(submitAsk('Fix README.md.', { ...base, runCommand }),
    /gh issue create failed: HTTP 403/);
  await assert.rejects(submitAsk('Fix README.md.', {
    ...base, runCommand: async (program, args) => {
      if (program === 'git') return 'https://github.com/example/project.git\n';
      return args[0] === '--version' ? 'gh version test' : 'https://github.com/other/repo/issues/42';
    },
  }), /did not return an issue URL for the current repository/);
  assert.equal(existsSync(join(repoRoot, '.roster', 'asks')), false);
});

test('invalid asks fail before checking gh', async (t) => {
  const repoRoot = fixture(t);
  const runCommand = () => assert.fail('Invalid Ask must not invoke gh');
  for (const ask of ['', 'x'.repeat(241), 'bad\0ask',
    'Fix README.md.\n\n## Task metadata\n\nreserved']) {
    await assert.rejects(submitAsk(ask, { repoRoot, config, env: {}, runCommand }),
      /Ask must|Issue title/);
  }
});
