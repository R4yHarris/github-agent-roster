import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import {
  cpSync, existsSync, mkdirSync, mkdtempSync, promises as fs, readFileSync, rmSync, symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { runIssue } from '../src/lib/issue.mjs';
import {
  formatRecommendation, inferTaskClass, loadLearning, recommend, recordRun,
} from '../src/lib/learn.mjs';
import { formatMetrics, loadMetrics, summarizeMetrics } from '../src/lib/metrics.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';

const exported = readFileSync(new URL('./fixtures/learn-metrics.jsonl', import.meta.url), 'utf8');
const runs = readFileSync(new URL('./fixtures/learn-runs.jsonl', import.meta.url), 'utf8');
const evaluations = readFileSync(new URL('./fixtures/learn-evals.jsonl', import.meta.url), 'utf8');
const contractsPath = resolveContractsPath();
const source = fileURLToPath(new URL('../src', import.meta.url));

function fixture(t, { learning = true } = {}) {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-learn-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  if (learning) {
    mkdirSync(join(cwd, '.roster', 'runs'), { recursive: true });
    writeFileSync(join(cwd, '.roster', 'runs', 'fixture.jsonl'), runs);
    writeFileSync(join(cwd, '.roster', 'evals.jsonl'), evaluations);
  }
  return cwd;
}

function metrics(cwd, options = {}) {
  return loadMetrics({ cwd, contractsPath, run: () => exported, ...options });
}

test('joins Git, local runs, and latest human evals without duplicating recorded runs', (t) => {
  const cwd = fixture(t);
  writeFileSync(join(cwd, '.roster', 'runs', 'duplicate.jsonl'), runs);
  const records = metrics(cwd);
  assert.equal(records.length, 13);
  const first = records[0];
  assert.equal(first.task_class, 'feat');
  assert.equal(first.model, 'careful-model');
  assert.equal(first.effort, 'h');
  assert.equal(first.context_used, 18);
  assert.equal(first.context_max, 200000);
  assert.equal(first.evaluation.verdict, 'accept');
  assert.equal(records.find((record) => record.session === 'pending-docs').context_out, 0);
  assert.equal(records.find((record) => record.session === 'unknown-model').model, undefined);
  const table = formatMetrics(summarizeMetrics(records));
  assert.match(table, /MODEL\s+EFFORT\s+RUNS\s+EVALS/);
  assert.match(table, /careful-model\s+h\s+6\s+5/);
  assert.match(table, /writer\s+-\s+1\s+0/);
  assert.match(table, /^-\s+-\s+1\s+0$/m);
});

test('keeps --ref restricted to exported history and lets --evals override matching SHA evals', (t) => {
  const cwd = fixture(t);
  writeFileSync(join(cwd, 'extra.jsonl'),
    '{"sha":"3333333333333333333333333333333333333333","verdict":"accept","difficulty":3,"again":true}\n');
  const records = metrics(cwd, {
    ref: 'main..HEAD',
    evalsPath: 'extra.jsonl',
    run(program, args, options) {
      assert.equal(program, process.execPath);
      assert.deepEqual(args.slice(1), ['--ref', 'main..HEAD']);
      assert.equal(options.cwd, cwd);
      return exported.split('\n').slice(0, 3).join('\n');
    },
  });
  assert.equal(records.length, 3);
  assert.equal(records[0].task_class, 'feat');
  assert.equal(records[2].evaluation.verdict, 'accept');
  assert.equal(recommend(records, 'feat').acceptRate, 1);
});

test('chooses the highest accept-rate with n >= 3, excluding unevaluated and duplicate session samples', (t) => {
  const result = recommend(metrics(fixture(t)), 'feat');
  assert.deepEqual(result, {
    model: 'careful-model', effort: 'h', n: 3, accepted: 2, acceptRate: 2 / 3,
  });
  assert.equal(formatRecommendation(result, 'feat'),
    'feat: careful-model effort=h accept-rate=66.7% n=3\n');
});

test('requires three evaluations of the same task class and model/effort pair', (t) => {
  const records = metrics(fixture(t));
  for (const taskClass of ['fix', 'docs', 'test']) {
    assert.equal(formatRecommendation(recommend(records, taskClass), taskClass), 'insufficient data\n');
  }
  const samples = records.filter((record) => ['feat-one', 'feat-two'].includes(record.session));
  assert.equal(recommend(samples, 'feat'), null);
  const exactlyThree = records.filter((record) => ['feat-one', 'feat-two', 'feat-three'].includes(record.session));
  assert.equal(recommend(exactlyThree, 'feat').n, 3);
  assert.equal(recommend(exactlyThree.map((record, index) => ({
    ...record, effort: index % 2 ? 'l' : 'h',
  })), 'feat'), null);
  assert.throws(() => recommend(records, 'chore'), /task class must be/);
});

test('uses deterministic ties, with larger evaluated samples ahead of model names', () => {
  const samples = (model, n) => Array.from({ length: n }, (_, index) => ({
    model, task: 'test-unit', evaluation: {
      session: `${model}-${index}`, verdict: 'accept',
    },
  }));
  assert.equal(recommend([...samples('a', 3), ...samples('z', 4)], 'test').model, 'z');
  assert.equal(recommend([...samples('z', 3), ...samples('a', 3)], 'test').model, 'a');
  assert.equal(recommend(samples('unknown', 3), 'test'), null);
  assert.equal(recommend(samples('builtin-stub', 3), 'test'), null);
});

test('recognizes only explicit classes and conventional task/title prefixes', () => {
  for (const taskClass of ['feat', 'fix', 'docs', 'test']) {
    for (const text of [taskClass, `${taskClass}-one`, `${taskClass}: title`, `${taskClass}(cli)!: title`]) {
      assert.equal(inferTaskClass(text), taskClass);
    }
  }
  for (const text of ['feature', 'fixture', 'testing', 'issue-42', 'Update docs', undefined]) {
    assert.equal(inferTaskClass(text), undefined);
  }
});

test('records only when the runs directory exists, with known fields and no invented zero counts', async (t) => {
  const cwd = fixture(t, { learning: false });
  const record = { session: 'run-one', task: 'issue-42' };
  assert.equal(await recordRun(record, { cwd, env: {} }), null);
  assert.equal(existsSync(join(cwd, '.roster')), false);
  mkdirSync(join(cwd, '.roster', 'runs'), { recursive: true });
  assert.deepEqual(await recordRun(record, { cwd, env: {} }), record);
  const known = await recordRun({ ...record, session: 'run-two', sha: 'c'.repeat(40) }, {
    cwd,
    env: {
      AI_MODEL: 'reported-model', AI_EFFORT: 'high', AI_CONTEXT_USED: '0',
      AI_CONTEXT_MAX: '9007199254740993',
    },
  });
  assert.deepEqual(known, {
    sha: 'c'.repeat(40), session: 'run-two', task: 'issue-42',
    model: 'reported-model', effort: 'h', context_used: 0, context_max: '9007199254740993',
  });
  assert.deepEqual(loadLearning({ cwd }).runs, [record, known]);
  assert.equal(existsSync(join(cwd, '.roster', 'evals.jsonl')), false);
});

test('completed seats create a run journal without inventing metrics or human feedback', async (t) => {
  const cwd = fixture(t, { learning: false });
  const record = { session: 'roster-42-planner', task: 'issue-42' };
  assert.deepEqual(await recordRun(record, { cwd, env: {}, createDirectory: true }), record);
  assert.deepEqual(loadLearning({ cwd }).runs, [record]);
  assert.equal(existsSync(join(cwd, '.roster', 'evals.jsonl')), false);
  const second = await recordRun({ session: 'roster-42-coder', task: 'issue-42' }, {
    cwd, createDirectory: true,
    env: { AI_MODEL: 'local-model', AI_EFFORT: 'high',
      AI_CONTEXT_USED: '4', AI_CONTEXT_OUT: '0' },
  });
  assert.deepEqual(second, {
    session: 'roster-42-coder', task: 'issue-42', model: 'local-model',
    effort: 'h', context_used: 4, context_out: 0,
  });
  assert.deepEqual(loadLearning({ cwd }).runs, [record, second]);
  await assert.rejects(recordRun(record, { cwd, createDirectory: 'yes' }), /createDirectory must be a boolean/);
  await assert.rejects(recordRun(record, {
    cwd, createDirectory: true, fileSystem: {
      ...fs, async mkdir() { throw new Error('denied'); },
    },
  }), /Could not create.*denied/);
});

test('automatic seat recording refuses a symlinked learning directory', async (t) => {
  const cwd = fixture(t, { learning: false });
  const outside = mkdtempSync(join(tmpdir(), 'roster-learn-outside-'));
  t.after(() => rmSync(outside, { recursive: true, force: true }));
  try {
    symlinkSync(outside, join(cwd, '.roster'), process.platform === 'win32' ? 'junction' : 'dir');
  } catch (error) {
    if (['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) {
      t.skip('Creating symlinks is unavailable on this system.');
      return;
    }
    throw error;
  }
  await assert.rejects(recordRun({ session: 'roster-42-coder' }, {
    cwd, env: {}, createDirectory: true,
  }), /symlinks/);
  assert.equal(existsSync(join(outside, 'runs')), false);
});

test('records effort and context even when the model is not reported', async (t) => {
  const cwd = fixture(t);
  const record = await recordRun({ session: 'partial', task: 'fix-one' }, {
    cwd, env: { AI_EFFORT: 'max', AI_CONTEXT_OUT: '7' },
  });
  assert.deepEqual(record, { session: 'partial', task: 'fix-one', effort: 'x', context_out: 7 });
});

test('appends a complete JSONL line to a file with no final newline', async (t) => {
  const cwd = fixture(t);
  const file = join(cwd, '.roster', 'runs', 'runs.jsonl');
  writeFileSync(file, '{"session":"previous"}');
  await recordRun({ session: 'next', task: 'docs-guide' }, { cwd, env: {} });
  assert.deepEqual(readFileSync(file, 'utf8').trimEnd().split('\n').map(JSON.parse), [
    { session: 'previous' }, { session: 'next', task: 'docs-guide' },
  ]);
});

function issueHarness(cwd, options = {}) {
  const calls = [];
  const messages = [];
  return {
    calls,
    messages,
    options: {
      cwd, env: {}, now: () => new Date('2026-09-28T12:00:00.000Z'),
      log: (message) => messages.push(message),
      async runCommand(program, args) {
        calls.push({ program, args });
        if (program === 'git' && args[0] === 'rev-parse') return cwd;
        if (program === 'git' && args[0] === 'remote') return 'https://github.com/example/repo.git';
        if (program === 'gh' && args[0] === 'issue') {
          return JSON.stringify({
            number: 42, title: 'fix(cli): handle input', body: 'The ask',
            url: 'https://github.com/example/repo/issues/42',
          });
        }
        if (program === 'git' && args[0] === 'worktree') {
          await fs.mkdir(args.at(-1), { recursive: true });
          return '';
        }
        throw new Error(`Unexpected command: ${program} ${args.join(' ')}`);
      },
      ...options,
    },
  };
}

test('successful roster run appends one record after assignment setup, never a human evaluation', async (t) => {
  const cwd = fixture(t, { learning: false });
  mkdirSync(join(cwd, '.roster', 'runs'), { recursive: true });
  const { options, calls } = issueHarness(cwd);
  const result = await runIssue('42', options);
  assert.deepEqual(loadLearning({ cwd }).runs, [
    { session: result.session, task: 'issue-42', task_class: 'fix' },
  ]);
  assert.equal(existsSync(join(cwd, '.roster', 'evals.jsonl')), false);
  assert.ok(calls.filter(({ program }) => program === 'gh').every(({ args }) => args[0] === 'issue'));
  assert.equal(readFileSync(result.envPath, 'utf8'),
    `AI_TASK=issue-42\nAI_SESSION=${result.session}\n`);
});

test('failed roster setup does not append a run or evaluation', async (t) => {
  const cwd = fixture(t, { learning: false });
  mkdirSync(join(cwd, '.roster', 'runs'), { recursive: true });
  const { options, messages } = issueHarness(cwd, {
    fileSystem: {
      ...fs,
      async writeFile() { throw new Error('assignment denied'); },
    },
  });
  await assert.rejects(runIssue('42', options), /assignment setup failed.*assignment denied/);
  assert.deepEqual(loadLearning({ cwd }), { runs: [], evaluations: [] });
  assert.deepEqual(messages, []);
});

test('surfaces run recording failures instead of announcing success', async (t) => {
  const cwd = fixture(t, { learning: false });
  mkdirSync(join(cwd, '.roster', 'runs'), { recursive: true });
  const { options, messages } = issueHarness(cwd, {
    fileSystem: {
      ...fs,
      async appendFile() { throw new Error('append denied'); },
    },
  });
  await assert.rejects(runIssue('42', options), /prepared but run recording failed.*append denied/);
  assert.deepEqual(messages, []);
});

test('rejects invalid known metadata and malformed local files with source context', async (t) => {
  const cwd = fixture(t);
  await assert.rejects(recordRun({ session: 'invalid' }, {
    cwd, env: { AI_MODEL: 'model', AI_CONTEXT_USED: '-1' },
  }), /Invalid AI-Run/);
  const runFile = join(cwd, '.roster', 'runs', 'fixture.jsonl');
  writeFileSync(runFile, '{"session":"s","context_used":-1}\n');
  assert.throws(() => metrics(cwd), /fixture\.jsonl:1: context_used must be/);
  writeFileSync(runFile, runs);
  writeFileSync(join(cwd, '.roster', 'evals.jsonl'), '{"session":"s","verdict":"accept","difficulty":6,"again":true}\n');
  assert.throws(() => metrics(cwd), /evals\.jsonl:1: difficulty must be/);
});

test('surfaces an invalid runs path and inspection errors', async (t) => {
  const cwd = fixture(t, { learning: false });
  mkdirSync(join(cwd, '.roster'));
  writeFileSync(join(cwd, '.roster', 'runs'), '');
  await assert.rejects(recordRun({ session: 's' }, { cwd, env: {} }), /must be a directory/);
  assert.throws(() => metrics(cwd), /Could not read.*runs/);
  await assert.rejects(recordRun({ session: 's' }, {
    cwd, env: {},
    fileSystem: { async stat() { throw Object.assign(new Error('denied'), { code: 'EACCES' }); } },
  }), /Could not inspect.*denied/);
});

test('CLI stats and recommend use the repository root, retain flags, and report insufficient data', (t) => {
  const cwd = fixture(t);
  execFileSync('git', ['init', '--quiet'], { cwd, stdio: 'pipe' });
  const roster = join(cwd, 'roster');
  cpSync(source, join(roster, 'src'), { recursive: true });
  const scripts = join(roster, 'vendor', 'github-agent-contracts', 'scripts');
  mkdirSync(scripts, { recursive: true });
  writeFileSync(join(scripts, 'agent-pr.mjs'), 'export {};\n');
  writeFileSync(join(scripts, 'export-agent-metrics.mjs'), `
if (process.argv[2] !== '--ref' || !['HEAD', 'main..HEAD'].includes(process.argv[3])) process.exit(9);
process.stdout.write(${JSON.stringify(exported)});
`);
  const nested = join(cwd, 'nested');
  mkdirSync(nested);
  writeFileSync(join(nested, 'extra.jsonl'),
    '{"sha":"3333333333333333333333333333333333333333","verdict":"accept","difficulty":3,"again":true}\n');
  const invoke = (args) => {
    const result = spawnSync(process.execPath, [join(roster, 'src', 'cli.mjs'), ...args], {
      cwd: nested, encoding: 'utf8', timeout: 10_000,
    });
    assert.ifError(result.error);
    return result;
  };
  const stats = invoke(['stats', '--ref', 'main..HEAD', '--evals', 'extra.jsonl']);
  assert.equal(stats.status, 0, stats.stderr);
  assert.match(stats.stdout, /careful-model\s+h\s+6\s+5/);
  assert.doesNotMatch(stats.stdout, /writer/);
  const suggestion = invoke(['recommend', '--task-class', 'feat']);
  assert.equal(suggestion.status, 0, suggestion.stderr);
  assert.equal(suggestion.stdout, 'feat: careful-model effort=h accept-rate=66.7% n=3\n');
  const insufficient = invoke(['recommend', '--task-class', 'docs']);
  assert.equal(insufficient.status, 0, insufficient.stderr);
  assert.equal(insufficient.stdout, 'insufficient data\n');

  const evalsFile = join(cwd, '.roster', 'evals.jsonl');
  const originalEvals = readFileSync(evalsFile, 'utf8');
  const twoEvals = [
    { session: 'feat-one', verdict: 'accept', difficulty: 3, again: true },
    { sha: '2222222222222222222222222222222222222222',
      verdict: 'accept', difficulty: 2, again: true },
  ];
  writeFileSync(evalsFile, `${twoEvals.map((evaluation) => JSON.stringify(evaluation)).join('\n')}\n`);
  const belowThreshold = invoke(['recommend', '--task-class', 'feat']);
  assert.equal(belowThreshold.status, 0, belowThreshold.stderr);
  assert.equal(belowThreshold.stdout, 'insufficient data\n');
  writeFileSync(evalsFile, `${twoEvals.map((evaluation) => JSON.stringify(evaluation)).join('\n')}\n` +
    '{"sha":"3333333333333333333333333333333333333333","verdict":"accept","difficulty":3,"again":true}\n');
  const atThreshold = invoke(['recommend', '--task-class', 'feat']);
  assert.equal(atThreshold.status, 0, atThreshold.stderr);
  assert.equal(atThreshold.stdout, 'feat: careful-model effort=h accept-rate=100.0% n=3\n');
  writeFileSync(evalsFile, originalEvals);

  for (const args of [
    ['recommend'], ['recommend', '--task-class', 'chore'],
    ['recommend', '--task-class', 'feat', '--extra'],
    ['stats', '--ref'], ['stats', '--evals'], ['stats', '--ref', 'HEAD', '--ref', 'HEAD'],
  ]) {
    assert.notEqual(invoke(args).status, 0);
  }
});
