import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, promises as fs, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { recordEvaluation } from '../src/lib/eval.mjs';
import { loadLearning } from '../src/lib/learn.mjs';

const cli = fileURLToPath(new URL('../src/cli.mjs', import.meta.url));
const evaluations = readFileSync(new URL('./fixtures/learn-evals.jsonl', import.meta.url), 'utf8');

function fixture(t) {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-eval-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const calls = [];
  return {
    cwd,
    calls,
    options: {
      cwd,
      run(program, args, options) {
        calls.push({ program, args, cwd: options.cwd });
        assert.equal(program, 'git');
        assert.deepEqual(args, ['rev-parse', '--show-toplevel']);
        return `${cwd}\n`;
      },
    },
  };
}

test('appends human accept/reject/rework decisions by full SHA or session', async (t) => {
  const { cwd, calls, options } = fixture(t);
  const records = [
    await recordEvaluation('A'.repeat(40), 'accept', '3', 'n', options),
    await recordEvaluation('human-session', 'reject', '1', 'y', options),
    await recordEvaluation('human-session', 'rework', '5', 'n', options),
    await recordEvaluation('B'.repeat(64), 'accept', '2', 'y', options),
  ];
  assert.deepEqual(records, [
    { sha: 'a'.repeat(40), verdict: 'accept', difficulty: 3, again: false },
    { session: 'human-session', verdict: 'reject', difficulty: 1, again: true },
    { session: 'human-session', verdict: 'rework', difficulty: 5, again: false },
    { sha: 'b'.repeat(64), verdict: 'accept', difficulty: 2, again: true },
  ]);
  assert.deepEqual(loadLearning({ cwd }).evaluations, records);
  assert.equal(existsSync(join(cwd, '.roster', 'runs')), false);
  assert.equal(calls.length, 4);
});

test('appends to fixture history without rewriting earlier human decisions', async (t) => {
  const { cwd, options } = fixture(t);
  mkdirSync(join(cwd, '.roster'));
  const file = join(cwd, '.roster', 'evals.jsonl');
  writeFileSync(file, evaluations.trimEnd());
  const record = await recordEvaluation('feat-one', 'rework', '4', 'n', options);
  assert.equal(readFileSync(file, 'utf8'), `${evaluations.trimEnd()}\n${JSON.stringify(record)}\n`);
  assert.equal(loadLearning({ cwd }).evaluations.at(-1).verdict, 'rework');
});

test('rejects invalid CLI values before reading git or creating local files', async (t) => {
  const { cwd, calls, options } = fixture(t);
  for (const args of [
    ['', 'accept', '3', 'y'], ['../outside', 'accept', '3', 'y'],
    ['has space', 'accept', '3', 'y'], ['-', 'accept', '3', 'y'],
    ['--option', 'accept', '3', 'y'], ['s'.repeat(65), 'accept', '3', 'y'],
    ['s', 'revise', '3', 'y'], ['s', 'Accept', '3', 'y'],
    ['s', 'accept', '0', 'y'], ['s', 'accept', '6', 'y'],
    ['s', 'accept', '2.5', 'y'], ['s', 'accept', '03', 'y'],
    ['s', 'accept', 'NaN', 'y'], ['s', 'accept', '3', 'yes'],
    ['s', 'accept', '3', 'Y'], ['s', 'accept', '3', true],
  ]) {
    await assert.rejects(recordEvaluation(...args, options));
  }
  assert.deepEqual(calls, []);
  assert.equal(existsSync(join(cwd, '.roster')), false);
});

test('resolves abbreviated commit IDs without interpreting them as command options', async (t) => {
  const { cwd, options } = fixture(t);
  const original = options.run;
  options.run = (program, args, commandOptions) => {
    if (args[1] === '--show-toplevel') return original(program, args, commandOptions);
    assert.equal(program, 'git');
    assert.equal(commandOptions.cwd, cwd);
    assert.deepEqual(args, ['rev-parse', '--verify', '--end-of-options', 'ABCDEF1^{commit}']);
    return `${'abcdef12'.repeat(5)}\n`;
  };
  assert.deepEqual(await recordEvaluation('ABCDEF1', 'accept', '3', 'n', options), {
    sha: 'abcdef12'.repeat(5), verdict: 'accept', difficulty: 3, again: false,
  });
});

test('does not save unresolved abbreviated SHAs or invalid Git output', async (t) => {
  const { cwd, options } = fixture(t);
  const original = options.run;
  for (const resolveCommit of [
    () => { throw new Error('unknown or ambiguous commit'); },
    () => 'not-a-sha',
  ]) {
    options.run = (program, args, commandOptions) => args[1] === '--show-toplevel'
      ? original(program, args, commandOptions)
      : resolveCommit();
    await assert.rejects(recordEvaluation('abcdef1', 'accept', '3', 'n', options),
      /Could not resolve commit|sha must be a full/);
  }
  assert.equal(existsSync(join(cwd, '.roster')), false);
});

test('surfaces malformed history and write errors without overwriting existing data', async (t) => {
  const { cwd, options } = fixture(t);
  mkdirSync(join(cwd, '.roster'));
  const file = join(cwd, '.roster', 'evals.jsonl');
  writeFileSync(file, 'not-json\n');
  await assert.rejects(recordEvaluation('s', 'accept', '3', 'n', options),
    /evals\.jsonl:1: invalid JSON/);
  assert.equal(readFileSync(file, 'utf8'), 'not-json\n');
  writeFileSync(file, evaluations);
  await assert.rejects(recordEvaluation('s', 'accept', '3', 'n', {
    ...options, fileSystem: { ...fs, async appendFile() { throw new Error('denied'); } },
  }), /Could not append.*evals\.jsonl.*denied/);
  assert.equal(readFileSync(file, 'utf8'), evaluations);
});

test('CLI evaluation writes at the repository root from a nested directory', (t) => {
  const { cwd } = fixture(t);
  execFileSync('git', ['init', '--quiet'], { cwd, stdio: 'pipe' });
  const nested = join(cwd, 'nested');
  mkdirSync(nested);
  const invoke = (args) => {
    const result = spawnSync(process.execPath, [cli, ...args], {
      cwd: nested, encoding: 'utf8', timeout: 10_000,
    });
    assert.ifError(result.error);
    return result;
  };
  const result = invoke(['eval', 'human-session', 'accept', '3', 'n']);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, 'Recorded AI-Eval for human-session.\n');
  assert.deepEqual(loadLearning({ cwd }).evaluations, [
    { session: 'human-session', verdict: 'accept', difficulty: 3, again: false },
  ]);
  assert.equal(existsSync(join(nested, '.roster')), false);
  for (const args of [
    ['eval'], ['eval', 's', 'accept', '3'], ['eval', 's', 'accept', '3', 'n', '--extra'],
    ['eval', 's', 'accept', '0', 'n'],
  ]) {
    const invalid = invoke(args);
    assert.notEqual(invalid.status, 0);
    assert.equal(invalid.stdout, '');
  }
  assert.equal(loadLearning({ cwd }).evaluations.length, 1);
});
