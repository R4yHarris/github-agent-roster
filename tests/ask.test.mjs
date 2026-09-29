import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { submitAsk } from '../src/lib/ask.mjs';

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
    number: 42, title: 'Add status to README.md.',
  });
  assert.deepEqual(calls.map(({ program, args }) => [program, args]), [
    ['gh', ['--version']],
    ['git', ['remote', 'get-url', 'origin']],
    ['gh', ['issue', 'create', '--repo', 'example/project',
      '--title', 'Add status to README.md.', '--body', ask]],
  ]);
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
  assert.equal(readFileSync(result.askPath, 'utf8'), '# Ask\n\nFix README.md.\n');
  assert.match(readFileSync(result.recipePath, 'utf8'), /ask: local:draft-1/);
  assert.match(readFileSync(result.taskPath, 'utf8'), /Fix README\.md/);
  assert.match(result.command, /^gh issue create --title .+ --body-file .+$/);
  assert.ok(result.command.includes(result.askPath));
  assert.deepEqual(calls, [['gh', ['--version']]]);
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
  for (const ask of ['', 'x'.repeat(241), 'bad\0ask']) {
    await assert.rejects(submitAsk(ask, { repoRoot, config, env: {}, runCommand }),
      /Ask must|Issue title/);
  }
});
