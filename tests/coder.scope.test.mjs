import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runCoder as runCoderSeat } from '../src/seats/coder.mjs';
import { withResearchSummary } from './helpers/research.mjs';

function runCoder(options) {
  return runCoderSeat({ ...options, fetchImpl: withResearchSummary(options.fetchImpl) });
}

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const stubConfig = parseConfig(example);

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

test('a scratch file the coder creates then deletes is not reported as out-of-scope work', async (context) => {
  const config = parseConfig(example.replace('profile: ""', 'profile: vllm-local')
    .replace('model: ""', 'model: served-model'));
  const options = fixture(context, config);
  let turns = 0;
  const call = (id, name, args) => ({ id, type: 'function', function: { name, arguments: JSON.stringify(args) } });
  const result = await runCoder({
    ...options, env: { AI_PROVIDER: 'github-copilot' },
    fetchImpl: async () => {
      turns += 1;
      const message = turns === 1 ? { role: 'assistant', tool_calls: [
        call('a', 'write_file', { path: 'scratch-explore.mjs', content: 'console.log(1);\n' }),
        call('b', 'write_file', { path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }),
      ] } : turns === 2 ? { role: 'assistant', tool_calls: [call('c', 'delete_file', { path: 'scratch-explore.mjs' })] }
        : { role: 'assistant', content: 'Added Status.' };
      return { status: 200, json: async () => ({ choices: [{ finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message }],
        usage: { prompt_tokens: 5, completion_tokens: 2 } }) };
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.equal(result.excellence.pass, true, result.excellence.reasons.join('; '));
  assert.deepEqual(result.scopeFiles, []);
  assert.doesNotMatch(readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8'), /scratch-explore/);
});
