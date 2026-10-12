import assert from 'node:assert/strict';
import { linkSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { captureVerificationSubject, judgeVerificationReceipt } from '../src/runtime/independent-verification.mjs';

const digest = 'a'.repeat(64);
const observation = (id = 1, check = 1) => ({
  id, check_id: check, expected_output: 'line 4', actual_output: 'line 4',
  expected_exit: 1, actual_exit: 1, evidence: 'Executed multiline diagnostic through the public operation',
});
const receipt = (observations = [observation()]) => ({
  schema_version: 1, subject_digest: digest, observations,
});
const options = { subjectDigest: digest, checkCount: 1 };

test('executed output and exit mismatches fail independently of any model verdict', () => {
  assert.equal(judgeVerificationReceipt(receipt(), options).status, 'pass');
  for (const change of [{ actual_output: 'line 3' }, { actual_exit: 0 }]) {
    const report = judgeVerificationReceipt(receipt([{ ...observation(), ...change }]), options);
    assert.equal(report.status, 'fail');
    assert.match(report.reasons[0], /observation 1, check 1 failed/);
  }
});

test('missing, stale, malformed, duplicate and incomplete receipts are refused', () => {
  const invalid = [
    undefined, null, {}, { ...receipt(), schema_version: 2 },
    { ...receipt(), subject_digest: 'b'.repeat(64) },
    { ...receipt(), verdict: 'pass' }, receipt([]),
    receipt([observation(), observation()]),
    receipt([{ ...observation(), check_id: 2 }]),
    receipt([{ ...observation(), actual_exit: null }]),
    receipt([{ ...observation(), actual_exit: NaN }]),
    receipt([{ ...observation(), expected_exit: -1 }]),
    receipt([{ ...observation(), evidence: 'no\nexecution' }]),
    receipt([{ ...observation(), evidence: ' ' }]),
    receipt([{ ...observation(), actual_output: 'x'.repeat(2001) }]),
    receipt([{ ...observation(), actual_output: '\0' }]),
    receipt([{ ...observation(), met: true }]),
    receipt(Array.from({ length: 65 }, (_, i) => observation(i + 1))),
  ];
  for (const value of invalid) assert.throws(() => judgeVerificationReceipt(value, options), /Independent verification/);
  assert.throws(() => judgeVerificationReceipt(receipt(), { ...options, checkCount: 2 }), /every acceptance check/);
});

test('multiple observations per check are allowed, but no contradiction is discarded', () => {
  const report = judgeVerificationReceipt(receipt([
    observation(), { ...observation(2), actual_output: 'line 3' }, observation(3, 2),
  ]), { subjectDigest: digest, checkCount: 2 });
  assert.equal(report.status, 'fail');
  assert.equal(report.observations.length, 3);
  assert.equal(report.reasons.length, 1);
});

test('subject binds exact task/result/product bytes, deletions and deterministic path order', async (context) => {
  const worktree = mkdtempSync(path.join(os.tmpdir(), 'roster-independent-subject-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true }));
  mkdirSync(path.join(worktree, 'src'));
  writeFileSync(path.join(worktree, 'TASK.md'), 'Original Ask and checks\n');
  writeFileSync(path.join(worktree, 'RESULT.md'), 'Executed tests\n');
  writeFileSync(path.join(worktree, 'src', 'app.mjs'), 'export const ready = true;\n');
  const files = ['src/app.mjs', 'src/deleted.mjs'];
  const before = await captureVerificationSubject({ worktree, files });
  assert.deepEqual(before, await captureVerificationSubject({ worktree, files: [...files].reverse() }));
  assert.equal(before.files[1].digest, null);
  for (const name of ['TASK.md', 'RESULT.md', 'src/app.mjs', 'src/deleted.mjs']) {
    writeFileSync(path.join(worktree, name), 'Changed bytes');
    assert.notEqual((await captureVerificationSubject({ worktree, files })).digest, before.digest);
  }
  for (const invalid of [[], ['../outside'], ['src/*'], ['src/app.mjs', 'src\\app.mjs'],
    ['TASK.md'], ['REVIEW.md'], ['.env'], [path.resolve(worktree, 'src', 'app.mjs')]]) {
    await assert.rejects(captureVerificationSubject({ worktree, files: invalid }), /Independent verification/);
  }
});

test('file byte limit and hardlink guards apply before accepting execution evidence', async (context) => {
  const worktree = mkdtempSync(path.join(os.tmpdir(), 'roster-independent-limits-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true }));
  writeFileSync(path.join(worktree, 'TASK.md'), 'Ask');
  writeFileSync(path.join(worktree, 'RESULT.md'), 'Tests');
  const source = path.join(worktree, 'app.mjs');
  writeFileSync(source, Buffer.alloc(65_536, 65));
  await captureVerificationSubject({ worktree, files: ['app.mjs'] });
  writeFileSync(source, Buffer.alloc(65_537, 65));
  await assert.rejects(captureVerificationSubject({ worktree, files: ['app.mjs'] }), /64 KiB/);
  writeFileSync(source, 'export const ready = true;');
  linkSync(source, path.join(worktree, 'alias.mjs'));
  await assert.rejects(captureVerificationSubject({ worktree, files: ['app.mjs'] }), /single-link/);
});
