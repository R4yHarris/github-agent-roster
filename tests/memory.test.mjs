import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { appendMemory, coderMemoryRecord, readMemory } from '../src/runtime/memory.mjs';

function fixture(context) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-memory-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  return { repoRoot, file: path.join(repoRoot, '.roster', 'memory', 'coder.jsonl'), env: {} };
}

test('appending twice preserves every old byte and tail returns both notebook records', async (context) => {
  const options = fixture(context);
  const first = coderMemoryRecord({
    task: 'issue-42', session: 'coder-42', mode: 'llm', time: '2026-01-01T00:00:00.000Z',
    changedFiles: ['README.md'], tests: { exit_code: 0 },
  });
  await appendMemory({ ...options, record: first });
  const prefix = readFileSync(options.file);
  const second = coderMemoryRecord({
    task: 'issue-43', session: 'coder-43', mode: 'stub', time: '2026-01-02T00:00:00.000Z',
  });
  await appendMemory({ ...options, record: second });
  assert.deepEqual(readFileSync(options.file).subarray(0, prefix.length), prefix);
  assert.deepEqual((await readMemory({ ...options, limit: 2 })).map(JSON.parse), [first, second]);
  assert.deepEqual((await readMemory({ ...options, limit: 1 })).map(JSON.parse), [second]);
  assert.deepEqual(await readMemory({ ...options, limit: 0 }), []);
  assert.equal(first.issue, 42);
  assert.match(first.changed, /README\.md/);
  assert.equal(first.tests, 'node --test exited 0');
  assert.equal(second.tests, 'Not run.');
  assert.match(second.next_gap, /Configure an LLM/);
});

test('tail crosses chunk and UTF-8 boundaries without loading or changing old records', async (context) => {
  const options = fixture(context);
  for (let index = 0; index < 40; index += 1) {
    await appendMemory({ ...options, record: { index, summary: '\u00e9'.repeat(400) } });
  }
  const before = readFileSync(options.file);
  const tail = (await readMemory({ ...options, limit: 3 })).map(JSON.parse);
  assert.deepEqual(tail.map(({ index }) => index), [37, 38, 39]);
  assert.ok(tail.every(({ summary }) => summary === '\u00e9'.repeat(400)));
  assert.deepEqual(readFileSync(options.file), before);
});

test('memory redacts credentials and rejects bulk bodies without persisting them', async (context) => {
  const options = fixture(context);
  await appendMemory({ ...options, env: { CUSTOM_KEY: 'test-only-value' }, apiKeyEnv: 'CUSTOM_KEY',
    record: { summary: 'Failed with test-only-value and token=not-a-real-token\nfull file contents here' } });
  const text = readFileSync(options.file, 'utf8');
  assert.doesNotMatch(text, /test-only-value|not-a-real-token|full file contents/);
  assert.match(text, /redacted/);
  assert.match(text, /details omitted/);
  const before = readFileSync(options.file);
  for (const record of [{ content: 'source code' }, { api_key: 'not-a-real-key' },
    { summary: { body: 'source code' } }, { summary: NaN }]) {
    await assert.rejects(appendMemory({ ...options, record }), /Memory/);
  }
  assert.deepEqual(readFileSync(options.file), before);
  writeFileSync(options.file, '{"summary":"legacy-secret"}\n');
  const sanitized = await readMemory({ ...options, env: { ROSTER_API_KEY: 'legacy-secret' } });
  assert.doesNotMatch(sanitized[0], /legacy-secret/);
  assert.equal(readFileSync(options.file, 'utf8'), '{"summary":"legacy-secret"}\n');
});

test('malformed tails, incomplete records, and invalid limits fail without rewriting the notebook', async (context) => {
  const options = fixture(context);
  assert.deepEqual(await readMemory(options), []);
  await appendMemory({ ...options, record: { summary: 'initial' } });
  writeFileSync(options.file, '{"summary":"incomplete"}');
  const original = readFileSync(options.file);
  await assert.rejects(appendMemory({ ...options, record: { summary: 'next' } }), /incomplete JSONL/);
  assert.deepEqual(readFileSync(options.file), original);
  for (const limit of [-1, 0.5, NaN]) {
    await assert.rejects(readMemory({ ...options, limit }), /tail limit/);
  }
  writeFileSync(options.file, '{bad\n');
  await assert.rejects(readMemory(options), /Invalid memory JSONL/);
  await assert.rejects(appendMemory({ ...options, file: path.join(options.repoRoot, '..', 'outside.jsonl'),
    record: { summary: 'outside' } }), /inside the roster repository/);
});

test('default seat notebooks are ignored by Git', (context) => {
  const options = fixture(context);
  copyFileSync(new URL('../.gitignore', import.meta.url), path.join(options.repoRoot, '.gitignore'));
  execFileSync('git', ['init', '--quiet'], { cwd: options.repoRoot, stdio: 'pipe' });
  assert.doesNotThrow(() => execFileSync('git', ['check-ignore', '--quiet', '--',
    '.roster/memory/coder.jsonl'], { cwd: options.repoRoot, stdio: 'pipe' }));
});
