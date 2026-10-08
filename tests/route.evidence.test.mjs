import assert from 'node:assert/strict';
import test from 'node:test';
import { chooseRoute, explainRouteEvidence, formatRoute } from '../src/lib/route.mjs';

const now = Date.parse('2026-10-07T00:00:00Z');
const profile = { id: 'fixture', model: 'fixture-model', base_url: 'https://fixture.invalid/v1',
  provider: 'vllm', context_max: 32768, concurrency: 1, hardware: 'unknown', task_class: ['fix'], notes: '' };
const options = { fleet: { profiles: [profile] }, capabilities: { capabilities: [] },
  taskClass: 'fix', difficulty: 2, now };
const row = (index, overrides = {}) => ({
  model: profile.model, task_class: 'fix', seat: 'coder', effort: 'h',
  evaluation: { session: `fixture-${index}`, verdict: 'accept', difficulty: 2,
    at: '2026-10-05T00:00:00Z', ...overrides },
});

test('no human evidence stays distinct from measured zero acceptance', () => {
  const empty = chooseRoute({ ...options, records: [] });
  assert.equal(empty.source, 'prior');
  assert.equal(empty.evidence[0].samples, 0);
  assert.equal(empty.evidence[0].acceptRate, null);
  assert.match(formatRoute(empty, 'fix'), /origin=none.*accept-rate=unknown.*warning=no-human-evidence/);
  const rejected = chooseRoute({ ...options, difficulty: 1, records: [row(0, { verdict: 'reject' })] });
  assert.equal(rejected.evidence[0].accepted, 0);
  assert.equal(rejected.evidence[0].rejected, 1);
  assert.equal(rejected.evidence[0].acceptRate, 0);
  assert.match(formatRoute(rejected, 'fix'), /origin=local-human-evaluations.*accept-rate=0.0%.*insufficient/);
});

test('three distinct scoped human samples explain the existing evidence choice without changing ranking', () => {
  const records = [row(1), row(2), row(3)];
  const choice = chooseRoute({ ...options, records: [...records, records[0]] });
  assert.equal(choice.source, 'evals');
  assert.equal(choice.recommendation.n, 3);
  assert.deepEqual(choice.evidence[0], {
    model: profile.model, seat: 'coder', task_class: 'fix', effort: 'h',
    samples: 3, accepted: 3, rejected: 0, acceptRate: 1, medianDifficulty: 2,
    latestEvaluation: '2026-10-05T00:00:00.000Z', ageDays: 2, sufficient: true,
  });
  assert.match(formatRoute(choice, 'fix'), /source=evals[\s\S]*age-days=2 warning=none/);
  assert.equal(chooseRoute({ ...options, records: records.slice(0, 2) }).source, 'prior');
});

test('seat, class and effort evidence remain separate and automated passes do not count', () => {
  const records = [row(1), { ...row(2), seat: 'reviewer' }, { ...row(3), task_class: 'docs' },
    { ...row(4), effort: 'l' }, { model: profile.model, task_class: 'fix', excellence: 'pass' }];
  const evidence = explainRouteEvidence(records, { model: profile.model, taskClass: 'fix', now });
  assert.equal(evidence.length, 2);
  assert.deepEqual(evidence.map(({ samples }) => samples), [1, 1]);
  assert.ok(evidence.every(({ sufficient }) => sufficient === false));
  assert.equal(chooseRoute({ ...options, records }).source, 'prior');
});

test('evidence never borrows another model and inferred task classes retain recency', () => {
  const inferred = { ...row(1), task_class: undefined, task: 'fix-scope' };
  const evidence = explainRouteEvidence([inferred, { ...row(2), model: 'another-model' }], {
    model: profile.model, taskClass: 'fix', now,
  });
  assert.equal(evidence[0].samples, 1);
  assert.equal(evidence[0].ageDays, 2);
  const lowDifficulty = [row(1, { difficulty: 1 }), row(2, { difficulty: 1 }), row(3, { difficulty: 1 })];
  const choice = chooseRoute({ ...options, records: lowDifficulty });
  assert.equal(choice.source, 'prior');
  assert.equal(choice.evidence[0].samples, 3);
  assert.equal(choice.evidence[0].sufficient, false);
  assert.match(formatRoute(choice, 'fix'), /insufficient-qualifying-evidence/);
});

test('defects derive rejects without rewriting human history and absent timestamps remain unknown', () => {
  const record = row(1, { at: undefined });
  record.defects = ['fixture defect'];
  const entry = explainRouteEvidence([record], { model: profile.model, taskClass: 'fix', now })[0];
  assert.equal(entry.rejected, 1);
  assert.equal(record.evaluation.verdict, 'accept');
  assert.equal(entry.latestEvaluation, null);
  assert.equal(entry.ageDays, null);
  for (const at of ['invalid', '2027-01-01T00:00:00Z']) {
    assert.equal(explainRouteEvidence([row(2, { at })], { model: profile.model, taskClass: 'fix', now })[0].ageDays, null);
  }
  assert.throws(() => explainRouteEvidence([], { model: profile.model, taskClass: 'fix', now: NaN }), /clock/);
});
