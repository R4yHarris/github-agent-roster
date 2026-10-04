import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { runCoder } from '../src/seats/coder.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:3456/v1')
  .replace('model: ""', 'model: local-model').replace('turn_budget: 8', 'turn_budget: 8'));

function fixture(context, difficulty) {
  const task = planStub('Update `README.md` with a Status section.', {
    reference: 'issue:4', metadata: { task_class: 'docs', difficulty },
  }).task;
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-level-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  mkdirSync(path.join(repoRoot, 'principals'));
  writeFileSync(path.join(repoRoot, 'principals', 'coder.md'),
    readFileSync(new URL('../principals/coder.md', import.meta.url), 'utf8'));
  cpSync(new URL('../skills/', import.meta.url), path.join(repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(worktree, 'AGENTS.md'), '# Instructions\nCode carefully.\n');
  writeFileSync(path.join(worktree, 'TASK.md'), task);
  writeFileSync(path.join(worktree, 'README.md'), '# Example\n');
  writeFileSync(path.join(worktree, '.roster'), 'not-a-directory\n');
  execFileSync('git', ['init', '--quiet'], { cwd: worktree });
  execFileSync('git', ['add', '--', '.'], { cwd: worktree });
  execFileSync('git', ['-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
    'commit', '--quiet', '-m', 'baseline'], { cwd: worktree });
  return { repoRoot, worktree, config, task: 'issue-4', session: 'roster-session' };
}

function response(finish_reason, message) {
  return { status: 200, json: async () => ({ choices: [{ finish_reason, message }] }) };
}

let writeIds = 0;
const writeCall = () => ({
  id: `write-${writeIds += 1}`, type: 'function', function: {
    name: 'write_file', arguments: JSON.stringify({
      path: 'README.md', content: '# Example\n\n## Status\nReady.\n',
    }),
  },
});

test('simple, medium, and hard one-file tasks finish on the same coder thread', async (context) => {
  const seen = [];
  for (const difficulty of [1, 3, 5]) {
    const options = fixture(context, difficulty);
    const prompts = [];
    const events = [];
    const result = await runCoder({
      ...options, env: {}, onEvent: async (event) => events.push(event),
      runTestCommand: async () => assert.fail('Docs-only task checks must not run node --test'),
      fetchImpl: async (_url, request) => {
        const body = JSON.parse(request.body);
        prompts.push(body.messages.at(-1).content);
        if (body.messages.at(-1).content.startsWith('Task checks passed after the sole allowed file was saved.')) {
          return response('stop', { role: 'assistant', content: 'Updated README Status.' });
        }
        return response('tool_calls', { role: 'assistant', content: null, tool_calls: [writeCall()] });
      },
    });
    assert.equal(result.excellence.pass, true, result.excellence.reasons.join('\n'));
    assert.match(readFileSync(path.join(options.worktree, 'README.md'), 'utf8'), /## Status/);
    assert.equal(events.some((event) => event.type === 'checkpoint' && event.status === 'unavailable'), true);
    assert.match(prompts.join('\n'), /Task checks passed after the sole allowed file was saved/);
    assert.doesNotMatch(prompts.join('\n'), /Repair \d+ of \d+/);
    seen.push(difficulty);
  }
  assert.deepEqual(seen, [1, 3, 5]);
});
