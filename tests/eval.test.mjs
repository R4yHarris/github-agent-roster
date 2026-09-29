import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, promises as fs, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  formatEvaluationResult, recordEvaluation, recordHumanEvaluation,
} from '../src/lib/eval.mjs';
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

function githubHarness(cwd, { associated = [{
  number: 7, html_url: 'https://github.com/example/repo/pull/7',
  base: { repo: { full_name: 'example/repo' } },
}], present = true, failComment = false } = {}) {
  const calls = [];
  return {
    calls,
    run(program, args, options) {
      calls.push({ program, args, options });
      if (program === 'git' && args[0] === 'rev-parse') return `${cwd}\n`;
      if (program === 'git' && args[0] === 'remote' && args.length === 1) return 'origin\n';
      if (program === 'git' && args[0] === 'remote' && args[1] === 'get-url') {
        return 'https://github.com/example/repo.git\n';
      }
      if (program === 'gh' && args[0] === '--version') {
        if (!present) throw Object.assign(new Error('gh missing'), { code: 'ENOENT' });
        return 'gh version 2.96\n';
      }
      if (program === 'gh' && args[0] === 'api') return JSON.stringify(associated);
      if (program === 'gh' && args[0] === 'pr') {
        if (failComment) throw new Error('comment denied');
        return 'https://github.com/example/repo/pull/7#issuecomment-1\n';
      }
      throw new Error(`Unexpected command: ${program} ${args.join(' ')}`);
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

test('human eval posts the contracts comment on the PR associated with a SHA', async (t) => {
  const { cwd } = fixture(t);
  const github = githubHarness(cwd);
  const result = await recordHumanEvaluation('A'.repeat(40), 'accept', '3', 'n', {
    cwd, run: github.run,
    env: { GITHUB_APP_ID: '123', GITHUB_APP_PRIVATE_KEY_PATH: 'test-only.pem' },
  });
  assert.deepEqual(result.evaluation, {
    sha: 'a'.repeat(40), verdict: 'accept', difficulty: 3, again: false,
  });
  assert.deepEqual(result.comment, { status: 'commented', number: 7 });
  assert.equal(formatEvaluationResult(result),
    `Recorded AI-Eval for ${'a'.repeat(40)}.\nCommented AI-Eval: 1|accept|3|n on PR #7.\n`);
  assert.deepEqual(loadLearning({ cwd }).evaluations, [result.evaluation]);
  assert.deepEqual(github.calls.find(({ program, args }) => program === 'gh' && args[0] === 'api').args,
    ['api', `repos/example/repo/commits/${'a'.repeat(40)}/pulls?per_page=100`]);
  assert.deepEqual(github.calls.find(({ program, args }) => program === 'gh' && args[0] === 'pr').args,
    ['pr', 'comment', '7', '--repo', 'example/repo', '--body', 'AI-Eval: 1|accept|3|n']);
  assert.equal(github.calls.filter(({ program }) => program === 'gh').every(({ options }) =>
    options.env.GITHUB_APP_ID === undefined && options.env.GITHUB_APP_PRIVATE_KEY_PATH === undefined), true);
});

test('session eval maps its exported run to a PR without posting an unrelated comment', async (t) => {
  const { cwd } = fixture(t);
  const github = githubHarness(cwd);
  const result = await recordHumanEvaluation('roster-42-coder', 'rework', '4', 'y', {
    cwd, run: github.run,
    metricsLoader: ({ cwd: root }) => {
      assert.equal(root, cwd);
      return [{ session: 'roster-42-coder', sha: 'b'.repeat(40) }];
    },
  });
  assert.equal(result.comment.number, 7);
  assert.deepEqual(loadLearning({ cwd }).evaluations, [result.evaluation]);
  assert.deepEqual(github.calls.find(({ program, args }) => program === 'gh' && args[0] === 'api').args,
    ['api', `repos/example/repo/commits/${'b'.repeat(40)}/pulls?per_page=100`]);
  assert.deepEqual(github.calls.find(({ program, args }) => program === 'gh' && args[0] === 'pr').args,
    ['pr', 'comment', '7', '--repo', 'example/repo', '--body', 'AI-Eval: 1|rework|4|y']);
});

test('offline or unpublished evaluations remain local with an explicit status', async (t) => {
  const { cwd } = fixture(t);
  const missingGh = githubHarness(cwd, { present: false });
  const offline = await recordHumanEvaluation('a'.repeat(40), 'accept', '3', 'n', {
    cwd, run: missingGh.run,
  });
  assert.deepEqual(offline.comment, { status: 'local', reason: 'gh is not installed' });
  assert.match(formatEvaluationResult(offline), /No PR comment: gh is not installed/);
  assert.equal(missingGh.calls.some(({ program, args }) => program === 'gh' && args[0] === 'api'), false);

  const noPr = githubHarness(cwd, { associated: [] });
  const unpublished = await recordHumanEvaluation('unpublished-session', 'reject', '2', 'n', {
    cwd, run: noPr.run, metricsLoader: () => [{ session: 'unpublished-session' }],
  });
  assert.deepEqual(unpublished.comment,
    { status: 'local', reason: 'no published commit for this session' });
  assert.equal(noPr.calls.some(({ program, args }) => program === 'gh' && args[0] === 'pr'), false);
  assert.equal(loadLearning({ cwd }).evaluations.length, 2);
});

test('ambiguous PRs and comment failures preserve local eval but do not claim publication', async (t) => {
  const { cwd } = fixture(t);
  const ambiguous = githubHarness(cwd, { associated: [
    { number: 7, html_url: 'https://github.com/example/repo/pull/7',
      base: { repo: { full_name: 'example/repo' } } },
    { number: 8, html_url: 'https://github.com/example/repo/pull/8',
      base: { repo: { full_name: 'example/repo' } } },
  ] });
  await assert.rejects(recordHumanEvaluation('a'.repeat(40), 'accept', '3', 'n', {
    cwd, run: ambiguous.run,
  }), /Recorded AI-Eval.*locally, but PR comment failed: Multiple PRs/);
  assert.equal(ambiguous.calls.some(({ program, args }) => program === 'gh' && args[0] === 'pr'), false);

  const denied = githubHarness(cwd, { failComment: true });
  await assert.rejects(recordHumanEvaluation('b'.repeat(40), 'reject', '5', 'y', {
    cwd, run: denied.run,
  }), /Recorded AI-Eval.*locally, but PR comment failed: comment denied/);
  assert.equal(loadLearning({ cwd }).evaluations.length, 2);
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
  assert.equal(result.stdout,
    'Recorded AI-Eval for human-session.\nNo PR comment: no GitHub origin.\n');
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
