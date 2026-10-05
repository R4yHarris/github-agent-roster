import assert from 'node:assert/strict';
import test from 'node:test';
import { addedLinesByFile, analyzeTestSubstance, isTestFile } from '../src/runtime/test-substance.mjs';

const header = `import assert from 'node:assert/strict';
import test from 'node:test';
import { chooseRoute, formatRoute } from '../src/lib/route.mjs';
const fleet = { profiles: [] };
function select(overrides = {}) {
  return chooseRoute({ fleet, taskClass: 'fix', difficulty: 3, ...overrides });
}
`;

function analyze(added, base = header) {
  return analyzeTestSubstance({ file: 'tests/route.test.mjs', text: `${base}\n${added}`, added });
}

test('test files are recognized by directory or suffix', () => {
  assert.equal(isTestFile('tests/route.test.mjs'), true);
  assert.equal(isTestFile('src\\lib\\route.spec.js'), true);
  assert.equal(isTestFile('test/helpers.mjs'), true);
  assert.equal(isTestFile('src/lib/route.mjs'), false);
});

test('a seeded sentinel passed to app code passes', () => {
  assert.deepEqual(analyze(`test('summary hides keys', () => {
  const secret = 'test-only-private-api-key';
  const route = formatRoute(select(), 'fix', {}, { ROSTER_API_KEY: secret });
  assert.ok(!JSON.stringify({ route }).includes(secret));
});`), []);
});

test('a sentinel injected through process.env or a tainted config object passes', () => {
  assert.deepEqual(analyze(`test('env injection', () => {
  process.env.ROSTER_API_KEY = 'test-only-private-api-key';
  const out = formatRoute(select(), 'fix');
  assert.doesNotMatch(out, /test-only-private-api-key/);
});`), []);
  assert.deepEqual(analyze(`test('config object', () => {
  const config = { llm: { api_key: 'test-only-private-api-key' } };
  const settings = { ...config, extra: true };
  const out = formatRoute(select(), 'fix', settings);
  assert.equal(out.includes('test-only-private-api-key'), false);
});`), []);
});

test('a sentinel removed by the test itself is flagged as unable to fail', () => {
  const reasons = analyze(`test('summary without secrets', () => {
  const summary = { route: select().profile.id, secrets: { apiKey: 'test-only-private-api-key' } };
  const publicSummary = { ...summary };
  delete publicSummary.secrets;
  assert.equal(JSON.stringify(publicSummary).includes('test-only-private-api-key'), false);
});`);
  assert.equal(reasons.length, 1);
  assert.match(reasons[0], /^Test substance: sentinel 'test-only-private-api-key' in tests\/route\.test\.mjs is asserted absent but never passed to app code/);
});

test('a sentinel kept on an unused record is flagged even when other app calls exist', () => {
  const reasons = analyze(`test('a run summary exposes the route and no credentials', () => {
  const choice = chooseRoute({ fleet, taskClass: 'fix', difficulty: 3 });
  const run = { profileId: choice.profile.id, credentials: { api_key: 'test-only-private-api-key' } };
  const summary = { profile: run.profileId, route: formatRoute(choice, 'fix') };
  const serialized = JSON.stringify(summary);
  assert.ok(!serialized.includes('test-only-private-api-key'));
  assert.ok(!serialized.includes('credentials'));
});`);
  assert.equal(reasons.length, 1);
  assert.match(reasons[0], /sentinel 'test-only-private-api-key'/);
});

test('a new test that never calls app code is flagged as tautological', () => {
  const reasons = analyze(`test('object shape', () => {
  const summary = { profile: 'fast' };
  assert.equal(summary.profile, 'fast');
});`);
  assert.deepEqual(reasons, ['Test substance: new test "object shape" in tests/route.test.mjs never calls imported app code; ' +
    'its assertions only inspect values the test built, so they cannot catch a regression.']);
});

test('helpers and namespace imports count as app calls; keyword scans are not sentinels', () => {
  assert.deepEqual(analyze(`test('helper call', () => {
  const out = formatRoute(select(), 'fix');
  assert.doesNotMatch(out, /api[_-]?key|token|secret/i);
});`), []);
  const base = `import test from 'node:test';\nimport assert from 'node:assert';\nimport * as route from '../src/lib/route.mjs';\n`;
  assert.deepEqual(analyze(`it('namespace', () => {\n  assert.ok(route.formatRoute({}, 'fix'));\n});`, base), []);
  assert.deepEqual(analyze(`test('only node builtins', () => {\n  assert.ok(true);\n});`, base).length, 1);
});

test('added lines are grouped per file from a zero-context diff', () => {
  const added = addedLinesByFile([
    'diff --git a/tests/a.test.mjs b/tests/a.test.mjs', '--- a/tests/a.test.mjs', '+++ b/tests/a.test.mjs',
    '@@ -1,0 +2,2 @@', '+one', '+two', '@@ -9 +11 @@', '-old', '+three',
    'diff --git a/src/x.mjs b/src/x.mjs', '@@ -1 +1 @@', '+x',
  ].join('\n'));
  assert.equal(added.get('tests/a.test.mjs'), ';\none\ntwo\n;\nthree');
  assert.equal(added.get('src/x.mjs'), ';\nx');
});
