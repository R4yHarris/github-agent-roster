import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { reviewedChecks } from './helpers/review.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { parseDefects, runReviewer } from '../src/seats/reviewer.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
  .replace('model: ""', 'model: review-model'));

const defect = { file: 'src/app.mjs', symbol: 'release', input: 'release() twice by one holder at capacity 2',
  outcome: 'a third holder is admitted beyond capacity' };

function fixture(context) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-reviewer-defects-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 }));
  mkdirSync(path.join(worktree, 'src'));
  writeFileSync(path.join(worktree, 'TASK.md'), '# Task: Update app\n\ndifficulty: 2\ntask_class: feat\n\n' +
    '## Acceptance checks\n- app exports ready\n\n## Files allowed\n- `src/app.mjs`\n\n## Ask\nUpdate app.\n');
  const source = path.join(worktree, 'src', 'app.mjs');
  writeFileSync(source, 'export const ready = false;\n');
  const git = (...args) => execFileSync('git', args, { cwd: worktree, encoding: 'utf8' });
  git('init', '-q', '-b', 'main');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-q', '-m', 'Fixture');
  writeFileSync(source, 'export const ready = true;\n');
  const resultPath = path.join(worktree, 'RESULT.md');
  writeFileSync(resultPath, '# Result\n\n## Verification\n\nChecks: PASS\n- node --test exited 0\n');
  return { worktree, repoRoot: worktree, coderResult: { mode: 'llm', model: 'review-model', resultPath,
    excellence: { pass: true, files: ['src/app.mjs'] } } };
}

function review(options, extra) {
  const seen = [];
  return runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      seen.push(body.messages[0].content);
      const content = JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
        checks: reviewedChecks(body).map((id) => ({ id, met: true, evidence: `Check ${id} is in the diff.` })),
        ...extra });
      return { status: 200, json: async () => ({ choices: [{ finish_reason: 'stop',
        message: { role: 'assistant', content } }] }) };
    } }).then((result) => ({ result, seen }));
}

test('a concrete defect fails the review even when every check is met', async (context) => {
  const { result, seen } = await review(fixture(context), { defects: [defect] });
  assert.equal(result.verdict, 'fail');
  assert.ok(result.reasons.includes(
    'Defect in src/app.mjs release: release() twice by one holder at capacity 2 -> a third holder is admitted beyond capacity'));
  assert.match(readFileSync(result.reviewPath, 'utf8'), /Verdict: fail[\s\S]*Defect in src\/app\.mjs release/);
  assert.match(seen[0], /bypasses the repository redaction path/);
  assert.match(seen[0], /releasable by the wrong holder/);
});

test('a reply without defects keeps the check-based verdict', async (context) => {
  const { result } = await review(fixture(context), {});
  assert.equal(result.verdict, 'pass', result.content);
});

test('defects must name a changed file and carry exactly four one-line fields', () => {
  assert.deepEqual(parseDefects(undefined, ['src/app.mjs']), []);
  assert.deepEqual(parseDefects([defect], ['src\\app.mjs']), [defect]);
  assert.throws(() => parseDefects([{ ...defect, file: 'src/other.mjs' }], ['src/app.mjs']), /changed file/);
  const { outcome, ...missing } = defect;
  assert.throws(() => parseDefects([missing], ['src/app.mjs']), /changed file/);
  assert.throws(() => parseDefects([{ ...defect, extra: 'x' }], ['src/app.mjs']), /changed file/);
  assert.throws(() => parseDefects([{ ...defect, input: '' }], ['src/app.mjs']), /changed file/);
  assert.throws(() => parseDefects(Array(9).fill(defect), ['src/app.mjs']), /at most 8/);
  assert.throws(() => parseDefects({}, ['src/app.mjs']), /at most 8/);
});
