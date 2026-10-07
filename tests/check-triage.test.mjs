import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { triageChecks, triageText } from '../src/runtime/check-triage.mjs';

function fixture(context) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-triage-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true }));
  mkdirSync(path.join(worktree, 'src'));
  mkdirSync(path.join(worktree, 'tests'));
  writeFileSync(path.join(worktree, 'src', 'store.mjs'), 'export function readRecords() {}\nconst hidden = 1;\nexport { hidden as visible };\n');
  writeFileSync(path.join(worktree, 'tests', 'store.test.mjs'), '');
  writeFileSync(path.join(worktree, 'tests', 'broken.test.mjs'), '');
  return worktree;
}

const runner = (calls) => async (command, args) => {
  calls.push(args.filter((arg) => arg.endsWith('.mjs')));
  if (args.some((arg) => arg.includes('broken'))) throw Object.assign(new Error('failed'), { code: 1 });
  return { stdout: 'ok 1 - passes\n' };
};

test('triage marks a passing command-only check met and records evidence', async (context) => {
  const worktree = fixture(context);
  const calls = [];
  const result = await triageChecks({ worktree, runCommand: runner(calls), checks: [
    '`node --test tests/store.test.mjs` exits 0',
    '`readRecords` from `src/store.mjs` returns every record',
  ] });
  assert.deepEqual(result.entries.map(({ status }) => status), ['met', 'unknown']);
  assert.match(result.entries[0].evidence[0], /already passes/);
  assert.match(result.entries[1].evidence[0], /`readRecords` is already exported from src\/store\.mjs/);
  assert.equal(result.allMet, false);
  assert.deepEqual(calls, [['tests/store.test.mjs']]);
  assert.match(triageText(result), /^Met checks need a pinning test[\s\S]*- met: `node --test tests\/store\.test\.mjs` exits 0/);
});

test('triage reports unmet checks for failing tests, missing files and missing exports', async (context) => {
  const worktree = fixture(context);
  const result = await triageChecks({ worktree, runCommand: runner([]), checks: [
    '`node --test tests/broken.test.mjs` exits 0',
    '`node --test tests/new.test.mjs` exits 0',
    '`writeRecords` from `src/store.mjs` persists records',
    '`visible` from `src/store.mjs` is re-exported',
    '`helper` from `src/missing.mjs` exists',
  ] });
  assert.deepEqual(result.entries.map(({ status }) => status), ['unmet', 'unmet', 'unmet', 'unknown', 'unmet']);
  assert.match(result.entries[0].evidence[0], /fails now/);
  assert.match(result.entries[1].evidence[0], /tests\/new\.test\.mjs does not exist yet/);
  assert.match(result.entries[2].evidence[0], /`writeRecords` is not exported/);
  assert.match(result.entries[4].evidence[0], /src\/missing\.mjs does not exist/);
});

test('triage sets allMet only when every check is verifiably met, and prose alone yields no text', async (context) => {
  const worktree = fixture(context);
  const calls = [];
  const allMet = await triageChecks({ worktree, runCommand: runner(calls), checks: [
    '`node --test tests/store.test.mjs` exits 0',
    '`node --test tests/store.test.mjs` exits 0.',
  ] });
  assert.equal(allMet.allMet, true);
  assert.equal(calls.length, 1, 'identical test commands run once');
  assert.match(triageText(allMet), /^Every acceptance check already holds[\s\S]*tests-only/);
  const prose = await triageChecks({ worktree, runCommand: runner([]), checks: ['Records survive a moved checkout', '`node --test` exits 0'] });
  assert.deepEqual(prose.entries.map(({ status }) => status), ['unknown', 'unknown']);
  assert.equal(prose.allMet, false);
  assert.equal(triageText(prose), '');
  assert.equal((await triageChecks({ worktree, runCommand: runner([]), checks: [] })).allMet, false);
});
