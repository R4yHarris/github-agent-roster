import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { passingReview, reviewedChecks } from './helpers/review.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { requirePassingReview, runReviewer } from '../src/seats/reviewer.mjs';
import { LlmTimeoutError } from '../src/llm/request.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
  .replace('model: ""', 'model: review-model'));

function fixture(context) {
  const base = mkdtempSync(path.join(tmpdir(), 'roster-reviewer-'));
  // A git diff killed on maxBuffer can briefly hold the directory on Windows after the promise rejects.
  context.after(() => rmSync(base, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 }));
  const repoRoot = path.join(base, 'roster');
  const worktree = path.join(base, 'task');
  mkdirSync(path.join(repoRoot, 'principals'), { recursive: true });
  cpSync(new URL('../principals/reviewer.md', import.meta.url),
    path.join(repoRoot, 'principals', 'reviewer.md'));
  mkdirSync(path.join(worktree, 'src'), { recursive: true });
  writeFileSync(path.join(worktree, 'TASK.md'),
    '# Task: Update app\n\ndifficulty: 4\ntask_class: feat\n\n## Acceptance checks\n- node --test exits 0\n' +
    '- app exports ready\n\n## Files allowed\n- `src/app.mjs`\n\n## Ask\nUpdate app.\n');
  const source = path.join(worktree, 'src', 'app.mjs');
  writeFileSync(source, 'export const ready = false;\n');
  const git = (...args) => execFileSync('git', args, { cwd: worktree, encoding: 'utf8' }).trim();
  git('init', '-b', 'main');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '-m', 'Fixture setup');
  writeFileSync(source, 'export const ready = true;\n');
  const resultPath = path.join(worktree, 'RESULT.md');
  writeFileSync(resultPath, '# Result\n\n## Verification\n\nChecks: PASS\n- node --test exited 0\n\n' +
    '## Run\n\nModel: review-model\n\n## Summary\n\nChanged app to export ready.\n');
  const coderResult = { mode: 'llm', model: 'review-model', resultPath,
    excellence: { pass: true, files: ['src/app.mjs'] } };
  return { worktree, repoRoot, source, coderResult, git };
}

test('reviewer reads the diff, RESULT, and acceptance checks without receiving any tools', async (context) => {
  const options = fixture(context);
  let requests = 0;
  const review = await runReviewer({
    ...options, config, env: { ROSTER_API_KEY: 'not-forwarded' },
    fetchImpl: async (url, request) => {
      requests += 1;
      assert.equal(String(url), 'http://localhost:1234/v1/chat/completions');
      const body = JSON.parse(request.body);
      assert.equal(body.tools, undefined);
      assert.deepEqual(body.response_format, { type: 'json_object' });
      assert.match(body.messages[0].content, /You are the builtin reviewer seat/);
      assert.match(body.messages[0].content, /Do not request `write_file`/);
      assert.match(body.messages[1].content, /app exports ready/);
      assert.match(body.messages[1].content, /Checks: PASS/);
      assert.match(body.messages[1].content, /-export const ready = false/);
      assert.match(body.messages[1].content, /\+export const ready = true/);
      assert.ok(request.headers.Authorization?.startsWith('Bearer '));
      assert.ok(!request.body.includes('not-forwarded'));
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: passingReview(body, { reasons: ['The diff implements the acceptance checks.'],
          security_notes: ['No secret or policy edits in the reviewed diff.'] }) } }],
        model: 'actual-review-model',
        usage: { prompt_tokens: 7, completion_tokens: 2 },
      }) };
    },
  });
  assert.equal(requests, 1);
  assert.equal(review.verdict, 'pass', review.content);
  assert.equal(review.queried, true);
  assert.deepEqual(review.usage, { prompt_tokens: 7, completion_tokens: 2 });
  assert.deepEqual(review.response, { model: 'actual-review-model',
    usage: { prompt_tokens: 7, completion_tokens: 2 } });
  assert.match(readFileSync(review.reviewPath, 'utf8'),
    /^# Review\n\nVerdict: pass[\s\S]*## Reasons[\s\S]*## Security notes/);
  assert.equal(readFileSync(options.source, 'utf8'), 'export const ready = true;\n');
  await requirePassingReview({ worktreePath: options.worktree, review });
  writeFileSync(review.reviewPath, review.content.replace('pass', 'fail'));
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review }),
    /changed after review/);
  await requirePassingReview({ worktreePath: options.worktree, review }, true);
  writeFileSync(review.reviewPath, review.content);
  writeFileSync(options.coderResult.resultPath,
    readFileSync(options.coderResult.resultPath, 'utf8').replace('Checks: PASS', 'Checks: FAIL'));
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review }),
    /RESULT\.md changed after review/);
});

test('a re-review judges the previous findings and is bound by TASK.md constraints', async (context) => {
  const options = fixture(context);
  const seen = [];
  const reviewer = (previousFindings) => runReviewer({ ...options, config, env: {}, previousFindings,
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      seen.push(body.messages);
      return { status: 200, json: async () => ({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: passingReview(body) } }] }) };
    } });
  assert.equal((await reviewer()).verdict, 'pass');
  assert.match(seen[0][0].content, /TASK\.md Constraints, which bind you too/);
  assert.match(seen[0][0].content, /never require something TASK\.md forbids/);
  assert.doesNotMatch(seen[0][0].content, /re-review/);
  assert.doesNotMatch(seen[0][1].content, /Previous review findings/);
  rmSync(path.join(options.worktree, 'REVIEW.md'));
  assert.equal((await reviewer(['Check 2 unmet: ready is not exported', ' '])).verdict, 'pass');
  assert.match(seen[1][0].content, /re-review after a repair[\s\S]*cite words from the check or TASK\.md/);
  assert.match(seen[1][1].content,
    /## RESULT\.md[\s\S]*## Previous review findings\n\n- Check 2 unmet: ready is not exported\n\n## Diff/);
});

test('a diff larger than 64 KiB but within seat.context_chars is reviewed, not a buffer failure', async (context) => {
  const options = fixture(context);
  const big = Array.from({ length: 3000 }, (_, index) => `export const value${index} = ${index}; // padding line`).join('\n');
  writeFileSync(options.source, `export const ready = true;\n${big}\n`);
  let seen = 0;
  const review = await runReviewer({ ...options, config, env: {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      seen = body.messages[1].content.length;
      return { status: 200, json: async () => ({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: passingReview(body) } }] }) };
    },
  });
  assert.ok(seen > 65_536);
  assert.equal(review.verdict, 'pass', review.content);
  rmSync(review.reviewPath);
  const tight = { ...config, seat: { ...config.seat, context_chars: 20_000 } };
  const refused = await runReviewer({ ...options, config: tight, env: {},
    fetchImpl: async () => assert.fail('An oversized diff must not reach the model') });
  assert.match(refused.content, /exceeds seat\.context_chars/);
  assert.doesNotMatch(refused.content, /maxBuffer/);
});

test('reviewer requires substantive public-path and seeded-secret evidence for test tasks', async (context) => {
  const options = fixture(context);
  writeFileSync(path.join(options.worktree, 'TASK.md'),
    '# Task: Add route-summary regression\n\ndifficulty: 2\ntask_class: test\n\n' +
    '## Acceptance checks\n- The public run summary exposes the selected route and profile\n' +
    '- Seed a credential-like sentinel and assert it is absent from serialized output\n' +
    '- node --test exits 0\n\n## Files allowed\n- `tests/route.test.mjs`\n\n' +
    '## Ask\nAdd a public run-summary regression test that proves the selected route/profile are visible without leaking secrets.\n');
  mkdirSync(path.join(options.worktree, 'tests'));
  writeFileSync(path.join(options.worktree, 'tests', 'route.test.mjs'), 'test("route summary", () => {});\n');
  options.coderResult.excellence.files = ['tests/route.test.mjs'];
  let systemInstructions;
  const review = await runReviewer({
    ...options, config, env: {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      systemInstructions = body.messages[0].content;
      return { status: 200, json: async () => ({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: passingReview(body),
      } }] }) };
    },
  });
  assert.equal(review.verdict, 'pass', review.content);
  assert.match(systemInstructions, /exercise the public operation when the Ask names one/);
  assert.match(systemInstructions, /exact sentinel is absent from\s+that code's serialized output/);
  assert.match(systemInstructions, /generic keyword scan, or a sentinel the test removes itself, is insufficient/);
  assert.match(systemInstructions, /built inside the test itself.*tautological/);
});

test('reviewer pass is derived from per-check evidence, not the model summary', async (context) => {
  const options = fixture(context);
  const checks = [];
  const rubberStamp = await runReviewer({
    ...options, config, env: {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      checks.push(...reviewedChecks(body));
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'pass', reasons: ['Looks complete.'], security_notes: [],
          checks: [{ id: 1, met: false, evidence: 'acquireLock is absent from the diff' },
            ...reviewedChecks(body).slice(1).map((id) => ({ id, met: true, evidence: 'shown by the diff' }))] }),
      } }] });
    },
  });
  assert.ok(checks.length >= 1);
  assert.equal(rubberStamp.verdict, 'fail');
  assert.match(rubberStamp.content, /Check 1 unmet: acquireLock is absent from the diff/);
  assert.match(rubberStamp.content, /## Acceptance checks\n\n- \[ \] 1\. /);
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review: rubberStamp }), /passing REVIEW\.md/);

  const unjudged = fixture(context);
  let calls = 0;
  const silent = await runReviewer({
    ...unjudged, config, env: {},
    fetchImpl: async (_url, request) => {
      calls += 1;
      if (calls === 2) assert.match(JSON.parse(request.body).messages.at(-1).content, /must judge every numbered acceptance check/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [] }) } }] });
    },
  });
  assert.equal(calls, 2);
  assert.equal(silent.verdict, 'fail');
  assert.match(silent.content, /Reviewer could not complete: .*must judge every numbered acceptance check/);

  const judged = fixture(context);
  const passed = await runReviewer({ ...judged, config, env: {},
    fetchImpl: async (_url, request) => Response.json({ choices: [{ finish_reason: 'stop', message: {
      role: 'assistant', content: passingReview(JSON.parse(request.body)) } }] }) });
  assert.equal(passed.verdict, 'pass', passed.content);
  assert.match(passed.content, /^# Review\n\nVerdict: pass\n[\s\S]*- \[x\] 1\. .+ — Check 1 is shown by the diff\./);
});

test('HTTP timeout cannot become a pass or completed review even if a caller supplies passing excellence', async (context) => {
  const options = fixture(context);
  const review = await runReviewer({ ...options, config, env: {},
    coderResult: { ...options.coderResult, error: new LlmTimeoutError({
      host: '192.168.1.48:8888', local: true, timeoutMs: 1_200_000, retryCommand: 'roster run --issue 108',
    }) },
    fetchImpl: () => assert.fail('Coder timeout must be rejected before reviewer model inference'),
  });
  assert.equal(review.verdict, 'fail');
  assert.equal(review.queried, false);
  assert.match(readFileSync(review.reviewPath, 'utf8'), /Verdict: fail[\s\S]*HTTP timeout[\s\S]*review was not completed/);
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review }), /passing REVIEW\.md/);
});
test('reviewer refuses a model-requested write_file under src and writes a failing report', async (context) => {
  const options = fixture(context);
  const review = await runReviewer({
    ...options, config, env: {},
    fetchImpl: async (_url, request) => {
      assert.equal(JSON.parse(request.body).tools, undefined);
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'tool_calls', message: {
          role: 'assistant', tool_calls: [{ id: 'write', type: 'function',
            function: { name: 'write_file', arguments: JSON.stringify({
              path: 'src/app.mjs', content: 'export const ready = false;\n',
            }) } }],
        } }],
      }) };
    },
  });
  assert.equal(review.verdict, 'fail');
  assert.match(review.content, /Reviewer cannot request tools/);
  assert.equal(readFileSync(options.source, 'utf8'), 'export const ready = true;\n');
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review }),
    /passing REVIEW\.md/);
});

test('stub and malformed reviewer responses fail without deleting coder work', async (context) => {
  const stub = fixture(context);
  const stubReview = await runReviewer({
    ...stub, config: parseConfig(example),
    coderResult: { ...stub.coderResult, mode: 'stub', excellence: { pass: false, files: [] } },
    fetchImpl: () => assert.fail('Stub review must not call a model'),
  });
  assert.equal(stubReview.verdict, 'fail');
  assert.equal(stubReview.queried, false);
  assert.match(stubReview.content, /Coder RESULT\.md has no passing implementation/);
  assert.match(stubReview.content, /Security review was not completed/);
  assert.equal(readFileSync(stub.source, 'utf8'), 'export const ready = true;\n');

  const malformed = fixture(context);
  let malformedCalls = 0;
  const malformedReview = await runReviewer({
    ...malformed, config, env: {},
    fetchImpl: async () => {
      malformedCalls += 1;
      return { status: 200, json: async () => ({
        choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: '{"verdict":"pass","reasons":[]}' } }],
      }) };
    },
  });
  assert.equal(malformedCalls, 2);
  assert.equal(malformedReview.verdict, 'fail');
  assert.match(malformedReview.content, /security_notes/);
  assert.equal(readFileSync(malformed.source, 'utf8'), 'export const ready = true;\n');
});

test('reviewer repairs malformed JSON once without rerunning the coder', async (context) => {
  const options = fixture(context);
  let calls = 0;
  const review = await runReviewer({
    ...options, config, env: {},
    fetchImpl: async (_url, request) => {
      calls += 1;
      const body = JSON.parse(request.body);
      if (calls === 1) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: 'The change looks good.',
        } }] });
      }
      assert.match(body.messages.at(-1).content, /Invalid reviewer JSON/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: passingReview(body),
      } }] });
    },
  });
  assert.equal(calls, 2);
  assert.equal(review.verdict, 'pass');
  assert.match(review.content, /Verdict: pass/);
  assert.equal(readFileSync(options.source, 'utf8'), 'export const ready = true;\n');
});

test('an incomplete diff or bounded-context failure cannot become a passing review', async (context) => {
  const options = fixture(context);
  const noDiff = await runReviewer({
    ...options, config,
    coderResult: { ...options.coderResult, excellence: { pass: true, files: [] } },
    fetchImpl: () => assert.fail('No diff must not contact a model'),
  });
  assert.equal(noDiff.verdict, 'fail');
  assert.match(noDiff.content, /task-allowed changed files/);

  const limited = fixture(context);
  const short = { ...config, seat: { ...config.seat, context_chars: 32 } };
  const limitedReview = await runReviewer({
    ...limited, config: short,
    fetchImpl: () => assert.fail('Truncated evidence must not be reviewed'),
  });
  assert.equal(limitedReview.verdict, 'fail');
  assert.match(limitedReview.content, /seat\.context_chars/);
});
