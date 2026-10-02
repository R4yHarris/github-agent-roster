import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createDebugLog, DebugLogError } from '../src/lib/debug-log.mjs';
import { createRunLog } from '../src/lib/run-log.mjs';
import { createBuiltinChat } from '../src/lib/llm.mjs';
import { createTools } from '../src/runtime/tools.mjs';
import { snapshotWorktree } from '../src/runtime/excellence.mjs';

const config = { llm: { base_url: '', model: '' } };

function fixture(t) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-debug-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  return repoRoot;
}

test('debug is off by default and does not create a file or logs directory', async (t) => {
  const repoRoot = fixture(t);
  const debug = createDebugLog({ env: {}, session: 'off-test' });
  const logger = await createRunLog({ repoRoot, session: 'roster-42-coder', env: {}, debug,
    errorOutput: { write() {} } });
  await logger.seat('coder', 'roster-42-coder', config, async (onEvent) => {
    await onEvent({ type: 'finish-reason', reason: 'length', retry: true });
    return {};
  });
  assert.equal(debug.enabled, false);
  assert.equal(debug.path, null);
  assert.equal(existsSync(path.join(repoRoot, '.roster', 'logs')), false);
  assert.equal(await debug.tail(), null);
});

test('a length finish and refused vendor list record only selected reason and path-class metadata', async (t) => {
  const repoRoot = fixture(t);
  const debug = createDebugLog({ env: {}, enabled: true, session: 'safe-test' });
  let shell = '';
  const logger = await createRunLog({ repoRoot, session: 'roster-42-coder', issue: 42, env: {}, debug,
    errorOutput: { write(text) { shell += text; } } });
  await logger.seat('coder', 'roster-42-coder', config, async (onEvent) => {
    await onEvent({ type: 'finish-reason', reason: 'length', retry: true, completion: 'PRIVATE_COMPLETION' });
    const tools = await createTools({ worktree: repoRoot, allowedFiles: ['README.md'],
      readmeOnlyDocs: true, onEvent });
    await assert.rejects(tools.list_dir({ path: 'vendor/private-directory' }), /Refused: outside the worktree\.$/);
    return {};
  });
  const text = readFileSync(debug.path, 'utf8');
  const rows = text.trim().split('\n').map((line) => JSON.parse(line));
  assert.ok(rows.some((row) => row.finish_reason === 'length' && row.phase === 'finish-retry'));
  const denied = rows.find((row) => row.phase === 'tool-denied');
  assert.equal(denied.tool_name, 'list_dir');
  assert.equal(denied.path_class, 'vendor');
  assert.equal(denied.issue, 42);
  assert.equal(denied.seat, 'coder');
  assert.doesNotMatch(text, /PRIVATE_COMPLETION|private-directory|"path":|"completion":/);
  assert.doesNotMatch(shell, /"time":|"path_class":/);
  assert.equal(shell.split('\n').filter((line) => line.includes('Listing')).length, 0);
  assert.equal(shell.split('\n').filter((line) => line === 'Refused: outside the worktree.').length, 1);
  assert.ok(rows.every((row) => Number.isInteger(row.elapsed_ms) && row.elapsed_ms >= 0));
});

test('debug off stops new lines and an explicit tail remains available', async (t) => {
  const repoRoot = fixture(t);
  const debug = createDebugLog({ env: { ROSTER_DEBUG: '1' }, session: 'toggle-test' });
  const context = { repoRoot, issue: 42, seat: 'coder', event: { type: 'seat-start' } };
  await debug.record(context);
  const first = readFileSync(debug.path, 'utf8');
  debug.setEnabled(false);
  await debug.record({ ...context, event: { type: 'seat-end' } });
  assert.equal(readFileSync(debug.path, 'utf8'), first);
  assert.equal((await debug.tail()).lines.length, 1);
  debug.setEnabled(true);
  await debug.record({ ...context, event: { type: 'seat-end' } });
  assert.equal((await debug.tail()).lines.length, 2);
  assert.equal(createDebugLog({ env: { ROSTER_DEBUG: 'true' } }).enabled, false);
});

test('JSONL never includes bodies, token counts, env values or PEM paths', async (t) => {
  const repoRoot = fixture(t);
  const debug = createDebugLog({ enabled: true, session: 'privacy-test',
    env: { TEST_VALUE: 'ENV_VALUE_DO_NOT_LOG', GITHUB_APP_PRIVATE_KEY_PATH: 'C:\\private\\secret.pem' } });
  const context = { repoRoot, issue: null, seat: 'coder' };
  await debug.record({ ...context, event: { type: 'tool', name: 'read_file', path: 'C:\\private\\secret.pem',
    prompt: 'PRIVATE_PROMPT', completion: 'PRIVATE_COMPLETION', content: 'PRIVATE_FILE_BODY',
    env: { TOKEN: 'PRIVATE_TOKEN' }, prompt_tokens: 123, completion_tokens: 456 } });
  await debug.record({ ...context, event: { type: 'finish-reason', reason: 'ENV_VALUE_DO_NOT_LOG', retry: false } });
  await debug.record({ ...context, event: { type: 'tool-result', name: 'run_test', status: 'ok', exit_code: 1,
    test_name: 'PRIVATE_TEST_BODY', stdout: 'PRIVATE_TEST_OUTPUT' } });
  await debug.record({ ...context, event: { type: 'test-repair', attempt: 1, budget: 4 } });
  await debug.record({ ...context, event: { type: 'finish-reason', reason: 'PRIVATE_COMPLETION', retry: false } });
  const text = readFileSync(debug.path, 'utf8');
  assert.doesNotMatch(text, /PRIVATE_|ENV_VALUE_DO_NOT_LOG|secret\.pem|private\\|prompt_tokens|completion_tokens/);
  const rows = text.trim().split('\n').map((line) => JSON.parse(line));
  assert.equal(rows[0].path_class, 'secret');
  assert.equal(rows[1].finish_reason, 'redacted');
  assert.equal(rows[2].test_name, 'node --test');
  assert.equal(rows[2].exit_code, 1);
  assert.deepEqual(rows[3].repair, { n: 1, of: 4 });
  assert.equal(rows[4].finish_reason, 'unsupported');
});

test('debug events from the actual chat retry exclude completion bodies and token usage', async (t) => {
  const repoRoot = fixture(t);
  const debug = createDebugLog({ env: {}, enabled: true, session: 'length-test' });
  const logger = await createRunLog({ repoRoot, session: 'roster-42-coder', issue: 42, env: {}, debug,
    errorOutput: { write() {} } });
  let calls = 0;
  await logger.seat('coder', 'roster-42-coder', config, async (onEvent) => {
    const chat = createBuiltinChat({ llm: { base_url: 'http://localhost:8000/v1', model: 'test-model', effort: 'l' } },
      { env: {}, onEvent, fetchImpl: async () => Response.json({
        choices: [{ finish_reason: ++calls === 1 ? 'length' : 'stop',
          message: { role: 'assistant', content: 'PRIVATE_RESPONSE_BODY' } }],
        usage: { prompt_tokens: 123, completion_tokens: 456 },
      }) });
    await chat({ messages: [{ role: 'user', content: 'PRIVATE_PROMPT_BODY' }] });
    return {};
  });
  const text = readFileSync(debug.path, 'utf8');
  assert.match(text, /"finish_reason":"length"/);
  assert.doesNotMatch(text, /PRIVATE_|prompt_tokens|completion_tokens/);
});

test('debug logs are Git-ignored, excluded from excellence snapshots, and denied to coder tools', async (t) => {
  const repoRoot = fixture(t);
  execFileSync('git', ['init', '--quiet'], { cwd: repoRoot });
  writeFileSync(path.join(repoRoot, '.gitignore'), '.roster/logs/\n');
  const before = await snapshotWorktree(repoRoot);
  const debug = createDebugLog({ env: {}, enabled: true, session: 'ignore-test' });
  await debug.record({ repoRoot, seat: 'coder', event: { type: 'seat-start' } });
  execFileSync('git', ['check-ignore', '--quiet', '--', debug.path], { cwd: repoRoot });
  assert.deepEqual(await snapshotWorktree(repoRoot), before);
  const tools = await createTools({ worktree: repoRoot, allowedFiles: ['**/*'] });
  await assert.rejects(tools.read_file({ path: '.roster/logs/debug-ignore-test.jsonl' }), /debug logs/);
  await assert.rejects(tools.write_file({ path: '.roster/logs/debug-ignore-test.jsonl', content: 'bad' }), /not allowed/);
});

test('tail refuses tampered content instead of printing unexpected fields', async (t) => {
  const repoRoot = fixture(t);
  const debug = createDebugLog({ env: {}, enabled: true, session: 'tamper-test' });
  await debug.record({ repoRoot, seat: 'coder', event: { type: 'seat-start' } });
  const row = JSON.parse(readFileSync(debug.path, 'utf8').trim());
  writeFileSync(debug.path, JSON.stringify({ ...row, prompt: 'PRIVATE_BODY' }) + '\n');
  await assert.rejects(debug.tail(), (error) => error instanceof DebugLogError && !error.message.includes('PRIVATE_BODY'));
});

test('an ignored directory does not permit writing a tracked debug file', async (t) => {
  const repoRoot = fixture(t);
  execFileSync('git', ['init', '--quiet'], { cwd: repoRoot });
  writeFileSync(path.join(repoRoot, '.gitignore'), '.roster/logs/\n');
  const logs = path.join(repoRoot, '.roster', 'logs');
  mkdirSync(logs, { recursive: true });
  const file = path.join(logs, 'debug-tracked-test.jsonl');
  writeFileSync(file, '{}\n');
  execFileSync('git', ['add', '--force', '--', file], { cwd: repoRoot });
  const debug = createDebugLog({ env: {}, enabled: true, session: 'tracked-test' });
  await assert.rejects(debug.record({ repoRoot, seat: 'coder', event: { type: 'seat-start' } }), /untracked and gitignored/);
  assert.equal(readFileSync(file, 'utf8'), '{}\n');
});
