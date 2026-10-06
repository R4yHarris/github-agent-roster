import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import {
  defaultFileBudgetMs, fileBudgetMs, listTestFiles, mergeTimings, orderTestFiles, overBudget, runTests, testJobs,
} from '../scripts/run-tests.mjs';

test('the scheduler starts unmeasured files first, then measured files slowest first', () => {
  assert.deepEqual(orderTestFiles(['tests/a.test.mjs', 'tests/b.test.mjs', 'tests/new.test.mjs', 'tests/c.test.mjs'],
    { 'tests/a.test.mjs': 10, 'tests/b.test.mjs': 300, 'tests/c.test.mjs': 40 }),
  ['tests/new.test.mjs', 'tests/b.test.mjs', 'tests/c.test.mjs', 'tests/a.test.mjs']);
});

test('workers default to all but one core and honor ROSTER_TEST_JOBS', () => {
  assert.equal(testJobs({}, 16), 15);
  assert.equal(testJobs({}, 1), 1);
  assert.equal(testJobs({ ROSTER_TEST_JOBS: '4' }, 16), 4);
  assert.equal(testJobs({ ROSTER_TEST_JOBS: 'zero' }, 16), 15);
  assert.equal(fileBudgetMs({}), defaultFileBudgetMs);
  assert.equal(fileBudgetMs({ ROSTER_TEST_FILE_BUDGET_MS: '5000' }), 5000);
});

test('timings merge new measurements, drop deleted files, and flag files over budget', () => {
  const merged = mergeTimings({ 'tests/gone.test.mjs': 5, 'tests/a.test.mjs': 100, 'tests/b.test.mjs': 7 },
    { 'tests/a.test.mjs': 90_000 }, (file) => file !== 'tests/gone.test.mjs');
  assert.deepEqual(merged, { 'tests/a.test.mjs': 90_000, 'tests/b.test.mjs': 7 });
  assert.deepEqual(overBudget(merged, 60_000), [['tests/a.test.mjs', 90_000]]);
});

test('listTestFiles finds only *.test.mjs files', (context) => {
  const directory = mkdtempSync(path.join(tmpdir(), 'roster-list-tests-'));
  context.after(() => rmSync(directory, { recursive: true, force: true }));
  for (const name of ['b.test.mjs', 'a.test.mjs', 'helper.mjs']) writeFileSync(path.join(directory, name), '');
  assert.deepEqual(listTestFiles(directory), ['tests/a.test.mjs', 'tests/b.test.mjs']);
});

test('runTests runs files in parallel, reports failures, and records timings', async (context) => {
  const directory = mkdtempSync(path.join(tmpdir(), 'roster-run-tests-'));
  context.after(() => rmSync(directory, { recursive: true, force: true }));
  const pass = path.join(directory, 'pass.test.mjs');
  const fail = path.join(directory, 'fail.test.mjs');
  writeFileSync(pass, "import test from 'node:test';\ntest('ok', () => {});\n");
  writeFileSync(fail, "import test from 'node:test';\ntest('broken thing', () => { throw new Error('nope'); });\n");
  const cache = path.join(directory, 'timings.json');
  let output = '';
  const code = await runTests({ files: [pass, fail], jobs: 2, budgetMs: 60_000, cache, write: (text) => { output += text; } });
  assert.equal(code, 1);
  assert.match(output, /ℹ tests 2\nℹ pass 1\nℹ fail 1/);
  assert.match(output, /fail\.test\.mjs: broken thing/);
  assert.deepEqual(Object.keys(JSON.parse(readFileSync(cache, 'utf8'))).sort(), [fail, pass].sort());
});
