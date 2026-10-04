import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, promises as fs, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { parseEvaluationArgs, recordEvaluation } from '../src/lib/eval.mjs';
import { parseShellEvaluationArgs } from '../src/shell/evaluation.mjs';
import { loadLearning } from '../src/lib/learn.mjs';

const cli = fileURLToPath(new URL('../src/cli.mjs', import.meta.url));
const evaluations = readFileSync(new URL('./fixtures/learn-evals.jsonl', import.meta.url), 'utf8');
const at = '2026-09-29T12:00:00.000Z';
const emptyMetadata = { sha: null, session: null, model: null, task_class: null,
  minutes: null, comment: '', at };

function fixture(t) {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-eval-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const calls = [];
  return {
    cwd,
    calls,
    options: {
      cwd, env: {}, now: () => new Date(at), metricsLoader: () => [], commenter: async () => {},
      run(program, args, options) {
        calls.push({ program, args, cwd: options.cwd });
        assert.equal(program, 'git');
        assert.deepEqual(args, ['rev-parse', '--show-toplevel']);
        return `${cwd}\n`;
      },
    },
  };
}

test('shell retrospective records one note and does not duplicate the session', async (t) => {
  const { cwd, options } = fixture(t);
  const parsed = parseShellEvaluationArgs('session-169 accept 1 n --minutes 15 --note "prefer a smaller diff next time; model stayed on scope"');
  const first = await recordEvaluation(...parsed.values, { ...options, ...parsed.options });
  assert.equal(first.verdict, 'accept');
  assert.equal(first.difficulty, 1);
  assert.equal(first.minutes, 15);
  assert.match(first.comment, /smaller diff/);
  assert.match(first.path, /evals\.jsonl$/);
  const second = await recordEvaluation(...parsed.values, { ...options, ...parsed.options });
  assert.equal(second.duplicate, true);
  assert.equal(readFileSync(join(cwd, '.roster', 'evals.jsonl'), 'utf8').trim().split('\n').length, 1);
  assert.throws(() => parseShellEvaluationArgs('session-169 reject 2 n --minutes 15'), /requires --note/);
  assert.throws(() => parseShellEvaluationArgs(' reject 2 n --minutes 15 --note "x"'), /Use \/eval/);
  assert.throws(() => parseShellEvaluationArgs('session-169 accept 6 n --minutes 15 --note "x"'), /Use \/eval/);
  assert.throws(() => parseShellEvaluationArgs('session-169 accept 1 n --note "x"'), /Use \/eval/);
  assert.equal(existsSync(join(cwd, '.roster', 'evals.jsonl')), true);
});

test('appends human accept/reject/rework decisions by full SHA or session', async (t) => {
  const { cwd, calls, options } = fixture(t);
  const records = [
    await recordEvaluation('A'.repeat(40), 'accept', '3', 'n', options),
    await recordEvaluation('human-session', 'reject', '1', 'y', options),
    await recordEvaluation('human-session', 'rework', '5', 'n', options),
    await recordEvaluation('B'.repeat(64), 'accept', '2', 'y', options),
  ].map(({ path, ...record }) => record);
  assert.deepEqual(records, [
    { ...emptyMetadata, sha: 'a'.repeat(40), verdict: 'accept', difficulty: 3, again: false },
    { ...emptyMetadata, session: 'human-session', verdict: 'reject', difficulty: 1, again: true },
    { ...emptyMetadata, session: 'human-session', verdict: 'rework', difficulty: 5, again: false },
    { ...emptyMetadata, sha: 'b'.repeat(64), verdict: 'accept', difficulty: 2, again: true },
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
  const { path, ...stored } = record;
  assert.equal(readFileSync(file, 'utf8'), `${evaluations.trimEnd()}\n${JSON.stringify(stored)}\n`);
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
    ...emptyMetadata, sha: 'abcdef12'.repeat(5), verdict: 'accept', difficulty: 3, again: false,
    path: join(cwd, '.roster', 'evals.jsonl'),
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
      cwd: nested, encoding: 'utf8', timeout: 10_000, env: { ...process.env, ROSTER_SEAT: undefined },
    });
    assert.ifError(result.error);
    return result;
  };
  const result = invoke(['eval', 'human-session', 'accept', '3', 'n',
    '--minutes', '12', '--comment', 'Ship quality.\nKeep the  tests.']);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, 'Recorded AI-Eval for human-session.\n');
  const [record] = loadLearning({ cwd }).evaluations;
  assert.equal(new Date(record.at).toISOString(), record.at);
  assert.deepEqual(record, { ...emptyMetadata, at: record.at,
    session: 'human-session', verdict: 'accept', difficulty: 3, again: false,
    minutes: 12, comment: 'Ship quality.\nKeep the  tests.' });
  assert.equal(readFileSync(join(cwd, '.roster', 'evals.jsonl'), 'utf8').trimEnd().split('\n').length, 1);
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

test('CLI and slash eval share flag validation and preserve quoted feedback', () => {
  const expected = { values: ['s', 'rework', '4', 'n'],
    options: { minutes: 20, comment: 'Keep the  edge-case test.' } };
  assert.deepEqual(parseEvaluationArgs(['s', 'rework', '4', 'n',
    '--minutes', '20', '--comment', 'Keep the  edge-case test.']), expected);
  assert.deepEqual(parseEvaluationArgs('s rework 4 n --minutes 20 --comment "Keep the  edge-case test."'), expected);
  assert.equal(parseEvaluationArgs("s rework 4 n --comment 'Try again' --minutes 2").options.minutes, 2);
  for (const suffix of ['--minutes -1', '--minutes 1.5', '--minutes 9007199254740992',
    '--minutes 2 --minutes 3', '--comment', '--comment "unclosed', '--unknown value']) {
    assert.throws(() => parseEvaluationArgs(`s rework 4 n ${suffix}`), /Use eval|minutes/);
  }
});

test('agent seats cannot write evaluations, and malformed feedback fails before any filesystem action', async (t) => {
  const { cwd, calls, options } = fixture(t);
  for (const seat of ['coder', 'planner']) {
    await assert.rejects(recordEvaluation('s', 'accept', '3', 'n',
      { ...options, env: { ROSTER_SEAT: seat } }), /human-only/);
  }
  for (const invalid of [{ minutes: -1 }, { minutes: '3' }, { comment: null }, { comment: '\0' },
    { comment: 'x'.repeat(4097) }]) {
    await assert.rejects(recordEvaluation('s', 'accept', '3', 'n', { ...options, ...invalid }), /minutes|comment/);
  }
  assert.deepEqual(calls, []);
  assert.equal(existsSync(join(cwd, '.roster')), false);
});

test('records actual run identity and posts the exact human AI-Eval and Minutes lines', async (t) => {
  const { cwd, options } = fixture(t);
  const sha = 'a'.repeat(40);
  const calls = [];
  const evaluation = await recordEvaluation('roster-42-coder', 'accept', '3', 'n', {
    ...options, minutes: 18, comment: 'Local retrospective only.', commenter: undefined,
    metricsLoader: () => [{ sha, session: 'roster-42-coder', model: 'served-model', task_class: 'fix' }],
    run(program, args, commandOptions) {
      calls.push([program, args]);
      assert.equal(commandOptions.cwd, cwd);
      if (program === 'git') return args[0] === 'rev-parse' ? cwd : 'https://github.com/example/project.git\n';
      if (args[0] === '--version') return 'gh version test\n';
      if (args[0] === 'api' && args[1] === 'user') return 'User\n';
      if (args[0] === 'api') return '[{"number":12}]';
      assert.deepEqual(args, ['pr', 'comment', '12', '--repo', 'example/project', '--body',
        'AI-Eval: 1|accept|3|n\nMinutes: 18']);
      return '';
    },
  });
  const { path, ...stored } = evaluation;
  assert.equal(path.endsWith('evals.jsonl'), true);
  assert.deepEqual(stored, {
    sha, session: 'roster-42-coder', model: 'served-model', task_class: 'fix',
    verdict: 'accept', difficulty: 3, again: false, minutes: 18, comment: 'Local retrospective only.', at,
  });
  assert.deepEqual(loadLearning({ cwd }).evaluations, [stored]);
  assert.equal(calls.filter(([program, args]) => program === 'gh' && args[1] === 'comment').length, 1);
});

test('missing gh is explicit and comment failures never erase the saved human decision', async (t) => {
  const { cwd, options } = fixture(t);
  const notices = [];
  const original = options.run;
  const run = (program, args, commandOptions) => {
    if (program === 'git') return original(program, args, commandOptions);
    throw Object.assign(new Error('gh not installed'), { code: 'ENOENT' });
  };
  await recordEvaluation('roster-42-coder', 'rework', '4', 'n', {
    ...options, run, commenter: undefined, minutes: 22, log: (line) => notices.push(line),
  });
  assert.match(notices[0], /unavailable.*saved locally/);
  await assert.rejects(recordEvaluation('roster-42-coder', 'accept', '4', 'y', {
    ...options, minutes: 20, commenter: async () => { throw new Error('not authenticated'); },
  }), /recorded locally.*PR comment failed.*not authenticated/);
  assert.equal(loadLearning({ cwd }).evaluations.length, 2);
});

test('a bot identity cannot post AI-Eval comments', async (t) => {
  const { cwd, options } = fixture(t);
  await assert.rejects(recordEvaluation('roster-42-coder', 'accept', '3', 'n', {
    ...options, commenter: undefined, minutes: 8,
    run(program, args) {
      if (program === 'git') return args[0] === 'rev-parse' ? cwd : 'https://github.com/example/project.git\n';
      if (args[0] === '--version') return 'gh version test\n';
      if (args[0] === 'api') return 'Bot\n';
      assert.deepEqual(args, ['pr', 'list', '--repo', 'example/project', '--head', 'issue-42',
        '--state', 'all', '--limit', '2', '--json', 'number']);
      return '[{"number":12}]';
    },
  }), /human GitHub identity/);
  assert.equal(loadLearning({ cwd }).evaluations.length, 1);
});

test('human evaluations retain an explicitly recorded seat for later ceiling derivation', async (t) => {
  const { cwd, options } = fixture(t);
  const result = await recordEvaluation('named-seat-session', 'reject', '2', 'n', {
    ...options, metricsLoader: () => [{ session: 'named-seat-session', seat: 'planner',
      model: 'seat-model', task_class: 'fix' }],
  });
  assert.equal(result.seat, 'planner');
  assert.equal(loadLearning({ cwd }).evaluations[0].seat, 'planner');
});
