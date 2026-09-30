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
import { recordEvaluation } from '../src/lib/eval.mjs';
import {
  formatRecommendation, inferTaskClass, joinLearning, loadLearning, median,
  parseRecommendationArgs, recommend, recordRun, summarizeLearning,
} from '../src/lib/learn.mjs';
import { formatMetrics, loadMetrics, summarizeMetrics } from '../src/lib/metrics.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { createBuiltinChat } from '../src/lib/llm.mjs';
import { buildRun } from '../src/metrics/run.mjs';

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

function capacitySamples(model, observations) {
  return observations.map((evaluation, index) => {
    const session = `${model}-${index}`;
    return { model, effort: 'h', task_class: 'fix', session,
      evaluation: { session, verdict: 'accept', difficulty: 4, again: true, ...evaluation } };
  });
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
  assert.match(table, /careful-model\s+h\s+5\s+4\s+feat\s+3/);
  assert.match(table, /careful-model\s+h\s+1\s+1\s+fix\s+1/);
  assert.match(table, /writer\s+-\s+1\s+0/);
  assert.match(table, /^-\s+-\s+1\s+0\s+-\s+0/m);
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
    medianMinutes: null, medianDifficulty: 3, estimate_min: null,
  });
  assert.equal(formatRecommendation(result, 'feat'),
    'feat: careful-model effort=h accept-rate=66.7% n=3 median-min=- median-difficulty=3\n');
});

test('requires three evaluations of the same task class and model/effort pair', (t) => {
  const records = metrics(fixture(t));
  for (const taskClass of ['fix', 'docs', 'test']) {
    assert.equal(formatRecommendation(recommend(records, taskClass), taskClass),
      'insufficient data; config default: (unset) effort=-\n');
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

test('capacity requires three distinct samples and the requested median difficulty before ranking acceptance', () => {
  const capable = capacitySamples('capable', [{ minutes: 30 }, { minutes: 10 }, { minutes: 20, difficulty: 5 }]);
  const easy = capacitySamples('easy', Array(3).fill({ minutes: 1, difficulty: 2 }));
  const harder = capacitySamples('harder', [
    { minutes: 5, difficulty: 5, verdict: 'reject' }, { minutes: 7, difficulty: 5 }, { minutes: 9, difficulty: 5 },
  ]);
  assert.equal(recommend(capable.slice(0, 2), 'fix', 4), null);
  assert.equal(recommend([capable[0], capable[1], capable[0]], 'fix', 4), null);
  assert.deepEqual(recommend([...easy, ...capable, ...harder], 'fix', 4), {
    model: 'capable', effort: 'h', n: 3, accepted: 3, acceptRate: 1,
    medianMinutes: 20, medianDifficulty: 4, estimate_min: 20,
  });
  const hard = recommend([...capable, ...harder], 'fix', 5);
  assert.equal(hard.model, 'harder');
  assert.equal(hard.estimate_min, 8);
  assert.equal(recommend(capable, 'fix', 5), null);
  for (const difficulty of [0, 6, 1.5, '4']) {
    assert.throws(() => recommend(capable, 'fix', difficulty), /difficulty must be/);
  }
  assert.deepEqual(parseRecommendationArgs(['--difficulty', '4', '--task-class', 'fix']),
    { taskClass: 'fix', difficulty: 4 });
  assert.equal(median([1, 2, 3, 4]), 2.5);
  assert.equal(median([]), null);
  assert.throws(() => median([1, NaN]), /finite numeric/);
  assert.equal(formatRecommendation(null, 'fix', { llm: { model: '', effort: 'h' } },
    { ROSTER_MODEL: 'environment-model' }),
  'insufficient data; config default: environment-model effort=h\n');
});

test('recorded excellence failures count as rejects without rewriting human evaluations', () => {
  const unsafe = capacitySamples('unsafe', Array(3).fill({ minutes: 10 }));
  unsafe[0].excellence = { pass: false, reasons: ['secret in diff'] };
  const clean = capacitySamples('clean', Array(3).fill({ minutes: 20 }));
  const group = summarizeLearning(unsafe)[0];
  assert.equal(group.n, 3);
  assert.equal(group.accepted, 2);
  assert.equal(group.acceptRate, 2 / 3);
  assert.equal(unsafe[0].evaluation.verdict, 'accept');
  assert.equal(recommend([...unsafe, ...clean], 'fix', 4).model, 'clean');
  const automated = summarizeLearning([
    { model: 'flagged', task_class: 'fix', session: 'failed', excellence: 'fail' },
    { model: 'flagged', task_class: 'fix', session: 'passed', excellence: 'pass' },
  ])[0];
  assert.equal(automated.n, 1);
  assert.equal(automated.accepted, 0);
  assert.equal(automated.medianDifficulty, null);
  assert.equal(automated.medianMinutes, null);
});

for (const file of ['.env', 'agent-policy.yml']) {
  test(`${file} defects survive later passing reports and human acceptance as rejects`, async (t) => {
    const cwd = fixture(t, { learning: false });
    const reason = `Diff path is protected or outside TASK.md allowed paths: ${file}`;
    for (let index = 0; index < 3; index += 1) {
      const run = { session: `unsafe-${index}`, model: 'unsafe', task_class: 'fix', effort: 'h' };
      await recordRun({ ...run, excellence: 'fail', defects: [reason] }, {
        cwd, env: {}, createDirectory: true,
      });
      await recordRun({ ...run, excellence: 'pass', defects: [] }, { cwd, env: {} });
      await recordEvaluation(run.session, 'accept', '4', 'y', {
        cwd, env: {}, run: () => cwd, minutes: 1,
        metricsLoader: () => joinLearning([], loadLearning({ cwd }).runs, []),
        commenter: async () => {},
      });
    }
    const history = loadLearning({ cwd });
    const joined = joinLearning([], history.runs, history.evaluations);
    assert.equal(joined.length, 3);
    assert.ok(joined.every(({ defects }) => defects.length === 1 && defects[0] === reason));
    assert.ok(history.evaluations.every(({ verdict }) => verdict === 'accept'));
    assert.deepEqual(summarizeLearning(joined).map(({ n, accepted, acceptRate }) =>
      ({ n, accepted, acceptRate })), [{ n: 3, accepted: 0, acceptRate: 0 }]);
    const clean = capacitySamples('clean', Array(3).fill({ minutes: 20 }));
    assert.equal(recommend([...joined, ...clean], 'fix', 4).model, 'clean');
    assert.equal(readFileSync(join(cwd, '.roster', 'runs', 'runs.jsonl'), 'utf8').trimEnd().split('\n').length, 6);
  });
}

test('defects remain rejects through Git joins and cannot be cleared by newer evaluations', () => {
  const sha = 'a'.repeat(40);
  const run = { sha, session: 'defective', model: 'known', task_class: 'fix', excellence: 'pass' };
  const reason = 'Diff path is protected or outside TASK.md allowed paths: .env';
  const records = joinLearning([run], [{ ...run, defects: [reason] }, { ...run, defects: [] }], [
    { sha, session: run.session, verdict: 'reject', difficulty: 4, again: false },
    { sha, session: run.session, verdict: 'accept', difficulty: 4, again: true, defects: [] },
  ]);
  assert.deepEqual(records[0].defects, [reason]);
  assert.equal(records[0].evaluation.verdict, 'accept');
  assert.equal(summarizeLearning(records)[0].accepted, 0);
  assert.equal(summarizeLearning([{ ...run, evaluation: null }])[0].accepted, 0);
});

test('run recording rejects malformed defects instead of silently losing their evidence', async (t) => {
  const cwd = fixture(t);
  for (const defects of ['policy edit', [null], [''], ['bad\0reason'], [{ reason: '.env' }]]) {
    await assert.rejects(recordRun({ session: 'invalid-defects', defects }, { cwd, env: {} }),
      /defects must be an array/);
  }
});

test('self-contained evals work without run exports and corrections never add samples', (t) => {
  const cwd = fixture(t, { learning: false });
  mkdirSync(join(cwd, '.roster'));
  const evidence = capacitySamples('from-evals', [{ minutes: 10 }, { minutes: 20 }, { minutes: 30 }])
    .map(({ model, effort, task_class, evaluation }) => ({ ...evaluation, model, effort, task_class }));
  const file = join(cwd, '.roster', 'evals.jsonl');
  writeFileSync(file, evidence.map((record) => JSON.stringify(record)).join('\n') + '\n');
  const records = metrics(cwd, { run: () => '' });
  assert.equal(records.length, 3);
  assert.equal(recommend(records, 'fix', 4).medianMinutes, 20);
  assert.equal(recommend(records, 'fix', 4).estimate_min, 20);
  assert.deepEqual(metrics(cwd, { run: () => '', ref: 'HEAD' }), []);
  writeFileSync(file, readFileSync(file, 'utf8') + JSON.stringify({ ...evidence[2], verdict: 'rework' }) + '\n');
  const corrected = recommend(metrics(cwd, { run: () => '' }), 'fix', 4);
  assert.equal(corrected.n, 3);
  assert.equal(corrected.accepted, 2);
  assert.equal(corrected.estimate_min, 15);
});

test('SHA-linked evaluations do not credit other commits and recorded failures survive duplicate run reports', async (t) => {
  const runs = ['a', 'b'].map((letter) => ({
    sha: letter.repeat(40), session: 'shared-session', model: 'known', task_class: 'fix',
  }));
  const evaluation = { sha: runs[0].sha, session: 'shared-session',
    verdict: 'accept', difficulty: 4, again: true };
  const joined = joinLearning(runs, [], [evaluation]);
  assert.equal(joined[0].evaluation, evaluation);
  assert.equal(joined[1].evaluation, null);
  const duplicate = joinLearning([], [
    { ...runs[0], excellence: 'fail' }, { ...runs[0], excellence: 'pass' },
  ], [evaluation]);
  assert.equal(summarizeLearning(duplicate)[0].accepted, 0);
  const cwd = fixture(t);
  await recordRun({ session: 'recorded-fail', model: 'known', task_class: 'fix',
    excellence: { pass: false, reasons: ['policy edit'] } }, { cwd, env: {} });
  assert.equal(loadLearning({ cwd }).runs.find(({ session }) => session === 'recorded-fail').excellence.pass, false);
  await assert.rejects(recordRun({ session: 'invalid', excellence: { pass: 'false' } }, { cwd, env: {} }),
    /excellence must be/);
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

test('vLLM and GitHub Copilot journals keep provider, model, and real usage without inventing a stub run', async (t) => {
  const cwd = fixture(t, { learning: false });
  const vllm = await recordRun({ session: 'vllm-session', task: 'fix-one', provider: 'vllm' }, {
    cwd, createDirectory: true,
    env: { AI_PROVIDER: 'local', AI_MODEL: 'served-model',
      AI_EFFORT: 'm', AI_CONTEXT_USED: '7', AI_CONTEXT_OUT: '0' },
  });
  const copilot = await recordRun({ session: 'copilot-session', task: 'fix-two' }, {
    cwd, env: { AI_PROVIDER: 'github-copilot', AI_MODEL: 'GPT-6-Sol',
      AI_EFFORT: 'h', AI_CONTEXT_USED: '9', AI_CONTEXT_OUT: '3' },
  });
  const rawVllm = await recordRun({ session: 'raw-vllm', task: 'fix-three' }, {
    cwd, env: { AI_PROVIDER: 'vllm', AI_MODEL: 'served-model', AI_CONTEXT_USED: '0' },
  });
  const stub = await recordRun({ session: 'stub', task: 'fix-four' }, {
    cwd, env: { AI_PROVIDER: 'github-copilot' },
  });
  assert.deepEqual(vllm, { session: 'vllm-session', task: 'fix-one', provider: 'vllm',
    model: 'served-model', effort: 'm', context_used: 7, context_out: 0 });
  assert.deepEqual(copilot, { session: 'copilot-session', task: 'fix-two', provider: 'github-copilot',
    model: 'GPT-6-Sol', effort: 'h', context_used: 9, context_out: 3 });
  assert.deepEqual(rawVllm, { session: 'raw-vllm', task: 'fix-three', provider: 'vllm',
    model: 'served-model', context_used: 0 });
  assert.deepEqual(stub, { session: 'stub', task: 'fix-four' });
  assert.deepEqual(loadLearning({ cwd }).runs, [vllm, copilot, rawVllm, stub]);
  const exported = [{ sha: 'a'.repeat(40), session: 'vllm-session', task: 'fix-one',
    provider: 'local', model: 'served-model' }];
  assert.equal(joinLearning(exported, [vllm], [])[0].provider, 'vllm');
  await assert.rejects(recordRun({ session: 'invalid-provider', provider: 'unauthenticated' }, {
    cwd, env: { AI_MODEL: 'served-model' },
  }), /provider must name a supported model backend/);
  assert.equal(loadLearning({ cwd }).runs.length, 4);
});

test('model-free journal normalization preserves large counts and rejects invalid partial evidence', async (t) => {
  const cwd = fixture(t);
  const record = await recordRun({ session: 'model-free' }, {
    cwd, env: { AI_EFFORT: '-', AI_CONTEXT_MAX: '9007199254740993', AI_CONTEXT_USED: '0' },
  });
  assert.deepEqual(record, { session: 'model-free', context_used: 0, context_max: '9007199254740993' });
  for (const env of [{ AI_EFFORT: 'invalid' }, { AI_CONTEXT_USED: '-1' }, { AI_CONTEXT_OUT: 4 }]) {
    await assert.rejects(recordRun({ session: 'invalid-partial' }, { cwd, env }), /AI_EFFORT|AI_CONTEXT/);
  }
  await assert.rejects(recordRun({ session: 'unknown-model', model: 'unknown' }, { cwd, env: {} }),
    /Invalid AI-Run model/);
});

test('journals the same live 100/40 metrics object and omits missing usage despite stale session fields', async (t) => {
  const secret = 'test-only-llm-secret';
  for (const usage of [{ prompt_tokens: 100, completion_tokens: 40 }, undefined]) {
    const cwd = fixture(t, { learning: false });
    const configured = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
      .replace('base_url: ""', 'base_url: http://localhost:8000/v1')
      .replace('model: ""', 'model: request-alias'));
    const config = { ...configured, llm: { ...configured.llm,
      provider: 'vllm', effort: 'h', api_key_optional: true } };
    const env = { AI_MODEL: 'GPT-6.1-Sol', AI_PROVIDER: 'github-copilot', AI_EFFORT: 'x',
      AI_CONTEXT_USED: '1000000', AI_CONTEXT_MAX: '1000000', AI_CONTEXT_OUT: '999',
      [config.llm.api_key_env]: secret };
    const chat = createBuiltinChat(config, { env, fetchImpl: async () => Response.json({
      model: 'actual-served-model', ...(usage ? { usage } : {}),
      choices: [{ message: { role: 'assistant', content: 'test-only-private-response' } }],
    }) });
    await chat({ messages: [{ role: 'user', content: 'test-only-private-prompt' }] });
    const run = buildRun({ config, response: chat.lastResponse, session: 'live-coder', task: 'issue-42', env });
    const stored = await recordRun({
      session: 'live-coder', task: 'issue-42', model: 'GPT-6.1-Sol', provider: 'github-copilot',
      effort: 'x', context_used: 1000000, context_max: 1000000, context_out: 999,
      prompt_tokens: 1000000, completion_tokens: 999,
    }, { cwd, env, run, createDirectory: true });
    assert.deepEqual(stored, run.metrics);
    assert.deepEqual(loadLearning({ cwd }).runs, [run.metrics]);
    assert.equal(stored.model, 'actual-served-model');
    assert.equal(stored.provider, 'vllm');
    assert.equal(Object.hasOwn(stored, 'context_max'), false);
    if (usage) {
      assert.equal(stored.prompt_tokens, 100);
      assert.equal(stored.completion_tokens, 40);
    } else {
      for (const field of ['prompt_tokens', 'completion_tokens', 'context_used', 'context_out']) {
        assert.equal(Object.hasOwn(stored, field), false);
      }
    }
    const raw = readFileSync(join(cwd, '.roster', 'runs', 'runs.jsonl'), 'utf8');
    for (const sensitive of [secret, 'test-only-private-prompt', 'test-only-private-response', '1000000']) {
      assert.ok(!raw.includes(sensitive));
    }
    await assert.rejects(recordRun({ session: 'different-session', task: 'issue-42' }, {
      cwd, env, run,
    }), /session\/task cannot be changed/);
    assert.equal(loadLearning({ cwd }).runs.length, 1);
  }
});

test('journal rejects invalid canonical token counts and inconsistent legacy aliases', async (t) => {
  const cwd = fixture(t, { learning: false });
  for (const fields of [{ prompt_tokens: -1 }, { completion_tokens: '40' },
    { prompt_tokens: 100, context_used: 1000000 }]) {
    await assert.rejects(recordRun({ session: 'bad-count', ...fields }, {
      cwd, env: {}, createDirectory: true,
    }), /safe integer|aliases must match/);
  }
  assert.deepEqual(loadLearning({ cwd }).runs, []);
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
          if (args[1] === 'list') return '';
          await fs.mkdir(args.at(-1), { recursive: true });
          return '';
        }
        if (program === 'git' && args[0] === 'for-each-ref') return '';
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

test('manual GHCP preparation journals declared attribution without stale used/output counts', async (t) => {
  const cwd = fixture(t, { learning: false });
  mkdirSync(join(cwd, '.roster', 'runs'), { recursive: true });
  const { options } = issueHarness(cwd, {
    config: parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
      .replace('model: ""', 'model: unrelated-served-model')),
    env: { AI_MODEL: 'GPT-6.1-Sol', AI_PROVIDER: 'local', AI_EFFORT: 'max',
      AI_CONTEXT_MAX: '1000000', AI_CONTEXT_USED: '1000000', AI_CONTEXT_OUT: '999' },
  });
  const result = await runIssue('42', options);
  assert.deepEqual(loadLearning({ cwd }).runs, [{
    session: result.session, task: 'issue-42', task_class: 'fix',
    model: 'GPT-6.1-Sol', provider: 'github-copilot', effort: 'x', context_max: 1000000,
  }]);
  assert.match(result.nextCommand, /--model GPT-6\.1-Sol --merge-when-green/);
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
  cpSync(new URL('../examples/', import.meta.url), join(roster, 'examples'), { recursive: true });
  writeFileSync(join(roster, 'roster.config.example.yml'),
    readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
      .replace('model: ""', 'model: fallback-model'));
  const scripts = join(roster, 'vendor', 'github-agent-contracts', 'scripts');
  mkdirSync(scripts, { recursive: true });
  writeFileSync(join(scripts, 'agent-pr.mjs'), 'export {};\n');
  writeFileSync(join(scripts, 'export-agent-metrics.mjs'), `
if (process.argv[2] !== '--ref' || !['HEAD', 'main..HEAD'].includes(process.argv[3])) process.exit(9);
process.stdout.write(${JSON.stringify(exported)});
`);
  writeFileSync(join(cwd, 'fixture.txt'), 'fixture\n');
  execFileSync('git', ['add', '--', 'fixture.txt'], { cwd, stdio: 'pipe' });
  execFileSync('git', ['-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '-m', 'Fixture setup'], { cwd, stdio: 'pipe' });
  writeFileSync(join(cwd, '.roster', 'fleet.yml'), formatFleet({ profiles: [
    { id: 'careful', base_url: 'https://careful.example.invalid/v1', model: 'careful-model',
      provider: 'vllm', context_max: 32768, concurrency: 1, hardware: 'test-gpu', notes: '' },
    { id: 'capacity', base_url: 'https://capacity.example.invalid/v1', model: 'cli-capacity',
      provider: 'vllm', context_max: 32768, concurrency: 1, hardware: 'test-gpu', notes: '' },
  ] }));
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
  assert.match(stats.stdout, /careful-model\s+h\s+5\s+4\s+feat\s+3/);
  assert.doesNotMatch(stats.stdout, /writer/);
  const suggestion = invoke(['recommend', '--task-class', 'feat']);
  assert.equal(suggestion.status, 0, suggestion.stderr);
  assert.match(suggestion.stdout,
    /^feat: careful-model effort=h accept-rate=66\.7% n=3 median-min=- median-difficulty=3 profile=careful source=evals /);
  const insufficient = invoke(['recommend', '--task-class', 'docs']);
  assert.equal(insufficient.status, 0, insufficient.stderr);
  const fallback = 'insufficient data; config default: fallback-model effort=m\n';
  assert.equal(insufficient.stdout, fallback);
  const tooHard = invoke(['recommend', '--task-class', 'feat', '--difficulty', '4']);
  assert.equal(tooHard.status, 0, tooHard.stderr);
  assert.equal(tooHard.stdout, fallback);

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
  assert.equal(belowThreshold.stdout, fallback);
  writeFileSync(evalsFile, `${twoEvals.map((evaluation) => JSON.stringify(evaluation)).join('\n')}\n` +
    '{"sha":"3333333333333333333333333333333333333333","verdict":"accept","difficulty":3,"again":true}\n');
  const atThreshold = invoke(['recommend', '--task-class', 'feat']);
  assert.equal(atThreshold.status, 0, atThreshold.stderr);
  assert.match(atThreshold.stdout, /^feat: careful-model effort=h accept-rate=100\.0% n=3 .*profile=careful source=evals /);
  const timed = capacitySamples('cli-capacity', [{ minutes: 10 }, { minutes: 20 }, { minutes: 30 }])
    .map(({ model, effort, task_class, evaluation }) => ({ ...evaluation, model, effort, task_class }));
  writeFileSync(evalsFile, timed.map((record) => JSON.stringify(record)).join('\n') + '\n');
  const capacityTable = invoke(['stats']);
  assert.equal(capacityTable.status, 0, capacityTable.stderr);
  assert.match(capacityTable.stdout, /cli-capacity\s+h\s+3\s+3\s+fix\s+3\s+100\.0%\s+20\s+4/);
  const capacity = invoke(['recommend', '--task-class', 'fix', '--difficulty', '4']);
  assert.equal(capacity.status, 0, capacity.stderr);
  assert.match(capacity.stdout,
    /^fix: cli-capacity effort=h accept-rate=100\.0% n=3 median-min=20 median-difficulty=4 profile=capacity source=evals /);
  writeFileSync(evalsFile, originalEvals);

  for (const args of [
    ['recommend'], ['recommend', '--task-class', 'chore'],
    ['recommend', '--task-class', 'feat', '--extra'],
    ['recommend', '--task-class', 'feat', '--difficulty', '0'],
    ['recommend', '--task-class', 'feat', '--difficulty', '6'],
    ['recommend', '--task-class', 'feat', '--difficulty'],
    ['stats', '--ref'], ['stats', '--evals'], ['stats', '--ref', 'HEAD', '--ref', 'HEAD'],
  ]) {
    assert.notEqual(invoke(args).status, 0);
  }
});
