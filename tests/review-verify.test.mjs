import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { passingReview } from './helpers/review.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { createRunLog, readLastRunLog } from '../src/lib/run-log.mjs';
import { endToEndCommands, maxReviewChars, runEndToEnd } from '../src/runtime/review-verify.mjs';
import { runReviewer } from '../src/seats/reviewer.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const config = parseConfig(example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')
  .replace('model: ""', 'model: review-model'));

function fixture(context, { checks = ['node --test exits 0', 'app exports ready'], cli } = {}) {
  const base = mkdtempSync(path.join(tmpdir(), 'roster-review-verify-'));
  context.after(() => rmSync(base, { recursive: true, force: true, maxRetries: 20, retryDelay: 250 }));
  const repoRoot = path.join(base, 'roster');
  const worktree = path.join(base, 'task');
  mkdirSync(path.join(repoRoot, 'principals'), { recursive: true });
  cpSync(new URL('../principals/reviewer.md', import.meta.url), path.join(repoRoot, 'principals', 'reviewer.md'));
  mkdirSync(path.join(worktree, 'src'), { recursive: true });
  writeFileSync(path.join(worktree, 'TASK.md'), '# Task: Update app\n\ndifficulty: 4\ntask_class: feat\n\n' +
    `## Acceptance checks\n${checks.map((check) => `- ${check}`).join('\n')}\n\n` +
    '## Files allowed\n- `src/app.mjs`\n\n## Ask\nUpdate app.\n');
  const source = path.join(worktree, 'src', 'app.mjs');
  writeFileSync(source, 'export const ready = false;\n');
  if (cli) writeFileSync(path.join(worktree, 'src', 'cli.mjs'), cli);
  const git = (...args) => execFileSync('git', args, { cwd: worktree, encoding: 'utf8' }).trim();
  git('init', '-b', 'main');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'Fixture setup');
  writeFileSync(source, 'export const ready = true;\n');
  const resultPath = path.join(worktree, 'RESULT.md');
  writeFileSync(resultPath, '# Result\n\n## Verification\n\nChecks: PASS\n- node --test exited 0\n\n' +
    '## Run\n\nModel: review-model\n\n## Summary\n\nChanged app to export ready.\n');
  const coderResult = { mode: 'llm', model: 'review-model', resultPath, excellence: { pass: true, files: ['src/app.mjs'] } };
  return { worktree, repoRoot, source, coderResult };
}

function reply(content) {
  return { status: 200, json: async () => ({
    choices: [{ finish_reason: 'stop', message: { role: 'assistant', content } }],
    model: 'review-model', usage: { prompt_tokens: 5, completion_tokens: 1 },
  }) };
}

test('end-to-end commands map only read-only roster paths from backticked checks', () => {
  const commands = endToEndCommands([
    'Running `roster history list --format json` prints valid JSON',
    'Running `roster ask "x"` and `roster run --issue 4` and `roster fleet probe a` are never executed',
    '`node src/cli.mjs recommend --task-class nope` exits non-zero with usage',
    '`roster status --issue 3` and `roster history list; rm -rf /` and `roster history list --format json`',
  ]);
  assert.deepEqual(commands.map(({ check, command, expectFailure }) => [check, command, expectFailure]), [
    [1, 'roster history list --format json', false],
    [3, 'roster recommend --task-class nope', true],
    [4, 'roster status --issue 3 --offline', false],
  ]);
  assert.deepEqual(endToEndCommands(['app exports ready']), []);
});

test('end-to-end runs real CLI paths with a credential-free sandbox and one error case', async () => {
  const seen = [];
  const runCommand = async (file, args, options) => {
    seen.push({ args, env: options.env });
    if (args.includes('--roster-review-invalid-option')) {
      throw Object.assign(new Error('usage'), { code: 1, stdout: '', stderr: 'Use roster history list' });
    }
    if (args[2] === 'crash') throw Object.assign(new Error('boom'), { code: 1, stderr: 'TypeError: x is undefined\n    at run (file:///w/src/cli.mjs:4:2)' });
    return { stdout: args[1] === 'stats' ? '{"ok":true}\n' : 'warning: legacy store\n{"records":[]}', stderr: '' };
  };
  const result = await runEndToEnd({
    worktree: process.cwd(), runCommand, env: { PATH: process.env.PATH, GITHUB_TOKEN: 'test-only-github-token' },
    checkTexts: ['`roster history list --format json` prints JSON', '`roster stats` prints JSON', '`roster status crash`'],
  });
  assert.equal(result.status, 'fail');
  assert.deepEqual(result.failures, [
    'End-to-end: `roster history list --format json` (check 1) stdout is not valid JSON.',
    'End-to-end: `roster status crash --offline` (check 3) crashed with a stack trace.',
  ]);
  assert.equal(seen.length, 6);
  for (const { env } of seen) {
    assert.equal(env.GITHUB_TOKEN, undefined);
    assert.match(env.GH_CONFIG_DIR, /roster-review-/);
    assert.equal(env.HOME, env.USERPROFILE);
  }
});

test('end-to-end runs the real roster CLI and fails an unexpected non-zero exit', async () => {
  const result = await runEndToEnd({ worktree: process.cwd(), checkTexts: [
    '`roster recommend --task-class feat` prints a recommendation',
    '`roster recommend --task-class nope` prints a recommendation',
  ] });
  assert.deepEqual(result.runs.map(({ exitCode }) => exitCode), [0, 1]);
  assert.deepEqual(result.runs.map(({ errorCase }) => errorCase.exitCode), [1, 1]);
  assert.deepEqual(result.failures, ['End-to-end: `roster recommend --task-class nope` (check 2) exited 1.']);
});

test('reviewer answers read-only requests and refuses writes', async (context) => {
  const options = fixture(context);
  const bodies = [];
  const review = await runReviewer({
    ...options, config, env: {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      bodies.push(body);
      assert.equal(body.tools, undefined);
      assert.deepEqual(body.response_format, { type: 'json_object' });
      if (bodies.length === 1) {
        assert.match(body.messages[0].content, /"requests"/);
        return reply(JSON.stringify({ requests: [
          { tool: 'write_file', path: 'src/app.mjs', content: 'export const ready = 1;\n' },
          { tool: 'read_file', path: 'src/app.mjs' },
          { tool: 'git_diff', path: 'src/app.mjs' },
        ] }));
      }
      return reply(passingReview(body));
    },
  });
  assert.equal(bodies.length, 2);
  const answer = bodies[1].messages.at(-1).content;
  assert.match(answer, /Refused: the reviewer is read-only/);
  assert.match(answer, /export const ready = true/);
  assert.match(answer, /\+export const ready = true/);
  assert.equal(readFileSync(options.source, 'utf8'), 'export const ready = true;\n');
  assert.equal(review.verdict, 'pass', review.content);
  assert.equal(review.reads, 3);
});

test('a failing end-to-end command fails the review even when the model passes it', async (context) => {
  const options = fixture(context, {
    checks: ['node --test exits 0', 'app exports ready', '`roster stats` prints JSON'],
    cli: "process.stdout.write('stats warning\\n{\"runs\":0}\\n');\nif (process.argv.length > 3) process.exit(1);\n",
  });
  const events = [];
  const review = await runReviewer({
    ...options, config, env: {}, onEvent: (event) => events.push(event),
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      assert.match(body.messages[1].content, /## Harness end-to-end runs/);
      assert.match(body.messages[1].content, /stats warning/);
      return reply(passingReview(body));
    },
  });
  assert.equal(review.verdict, 'fail');
  assert.equal(review.reasons[0], 'End-to-end: `roster stats` (check 3) stdout is not valid JSON.');
  assert.equal(review.endToEnd.status, 'fail');
  const event = events.find(({ type }) => type === 'review-e2e');
  assert.deepEqual(event, { type: 'review-e2e', status: 'fail', commands: 1, failures: 1 });
  let text = '';
  const log = { repoRoot: options.repoRoot, session: 'roster-7-reviewer', env: {}, errorOutput: { write(value) { text += value; } } };
  const logger = await createRunLog(log);
  await logger.seat('reviewer', log.session, { llm: { base_url: '', model: '' } }, async (onEvent) => {
    await onEvent(event);
    assert.match((await readLastRunLog(log)).lastLine, /seat reviewer review-e2e fail commands=1 failures=1$/);
    await onEvent({ type: 'review-reads', count: 3, refused: 1 });
    assert.match((await readLastRunLog(log)).lastLine, /seat reviewer review-reads count=3 refused=1$/);
  });
  assert.match(text, /Reviewer end-to-end run found 1 failing CLI paths; review fails\./);
});

test('reviewer output beyond the schema or the length cap is rejected', async (context) => {
  const options = fixture(context);
  let calls = 0;
  const review = await runReviewer({
    ...options, config, env: {},
    fetchImpl: async (_url, request) => {
      calls += 1;
      const passing = JSON.parse(passingReview(JSON.parse(request.body)));
      return reply(calls === 1
        ? JSON.stringify({ ...passing, reasons: Array(16).fill('x'.repeat(400)) })
        : JSON.stringify({ ...passing, essay: 'extra prose' }));
    },
  });
  assert.equal(calls, 2);
  assert.equal(review.verdict, 'fail');
  assert.equal(review.completed, false);
  assert.match(review.reasons[0], /Reviewer could not complete: Reviewer response must include a verdict/);
  assert.ok(maxReviewChars < 16 * 400);
});
