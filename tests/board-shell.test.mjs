import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { listOpenIssues, readDiffNames } from '../src/lib/board.mjs';
import { createDispatcher } from '../src/repl.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

test('open issue listing requests numbers and titles only and never prints tokens or bodies', async () => {
  const calls = [];
  const issues = await listOpenIssues({ root: process.cwd(), env: {}, runCommand: async (program, args) => {
    calls.push([program, args]);
    return program === 'git' ? 'https://github.com/example/project.git' : JSON.stringify([
      { number: 108, title: 'A public title', body: 'PRIVATE_ISSUE_BODY' },
    ]);
  } });
  assert.deepEqual(issues, [{ number: 108, title: 'A public title', state: 'OPEN' }]);
  assert.equal(calls[1][1].at(-1), 'number,title');
  let text = '';
  const shell = createDispatcher({ config, env: { ROSTER_API_KEY: 'token-test-marker' },
    output: { write(value) { text += value; } }, errorOutput: { write() {} },
    services: { repositoryBranch: () => 'main', repositoryRoot: () => process.cwd(),
      listOpenIssues: async () => [{ number: 108, title: 'token-test-marker', body: 'PRIVATE_BODY', state: 'OPEN' }],
      readStatus: () => assert.fail('Issue list cache must avoid GitHub on /issue') } });
  await shell.dispatch('/issues');
  await shell.dispatch('/issue 108');
  assert.doesNotMatch(text, /token-test-marker|PRIVATE_BODY/);
  assert.match(text, /#108 \[redacted\]/);
  assert.match(text, /State: OPEN\nBranch: issue-108\nPR: \(not cached\)/);
});

test('issue metadata is fetched once then read from cache', async () => {
  let reads = 0;
  let text = '';
  const shell = createDispatcher({ config, env: {}, output: { write(value) { text += value; } },
    errorOutput: { write() {} }, services: { repositoryBranch: () => 'main', repositoryRoot: () => process.cwd(),
      readStatus: async () => {
        reads += 1;
        return { issue: { number: 108, title: 'Cached title', state: 'OPEN', body: 'PRIVATE_BODY' },
          branch: 'issue-108', openPr: { url: 'https://github.com/example/project/pull/5' } };
      } } });
  await shell.dispatch('/issue 108');
  await shell.dispatch('/issue 108');
  assert.equal(reads, 1);
  assert.match(text, /Cached title/);
  assert.match(text, /PR: https:\/\/github\.com\/example\/project\/pull\/5/);
  assert.doesNotMatch(text, /PRIVATE_BODY/);
});

test('diff invokes the filename-only Git form and does not expose changed file content', async (t) => {
  const root = mkdtempSync(path.join(tmpdir(), 'roster-board-diff-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' });
  git('init', '--quiet', '-b', 'main');
  writeFileSync(path.join(root, 'README.md'), '# Before\n');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'Fixture');
  writeFileSync(path.join(root, 'README.md'), 'PRIVATE_CHANGED_FILE_BODY\n');
  assert.deepEqual(await readDiffNames({ cwd: root }), ['README.md']);
  let text = '';
  const shell = createDispatcher({ cwd: root, config, env: {}, output: { write(value) { text += value; } },
    errorOutput: { write() {} }, services: { repositoryRoot: () => root, repositoryBranch: () => 'main' } });
  shell.state.lastRun = { worktreePath: root, repoRoot: root };
  await shell.dispatch('/diff');
  assert.equal(text, 'README.md\n');
  assert.doesNotMatch(text, /PRIVATE_CHANGED_FILE_BODY/);
});

test('debug tail fails closed while disabled before reading any JSONL', async () => {
  let reads = 0;
  const debug = { enabled: false, setEnabled(value) { this.enabled = value; },
    async tail() { reads += 1; return { lines: ['safe metadata'] }; } };
  const shell = createDispatcher({ config, env: {}, debug, services: { repositoryBranch: () => 'main' },
    output: { write() {} }, errorOutput: { write() {} } });
  await assert.rejects(shell.dispatch('/log debug'), /Debug logging is off/);
  assert.equal(reads, 0);
  await shell.dispatch('/debug on');
  await shell.dispatch('/log debug');
  assert.equal(reads, 1);
});
