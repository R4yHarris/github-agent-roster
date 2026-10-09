// Builtin seat orchestration: Perspective escalation, re-scoping, review repair, and review carryover.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { passingReview, reviewedChecks } from './helpers/review.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { writeAsk } from '../src/lib/ask.mjs';
import {
  coderStuckReason, maxPerspectiveEscalations, perspectiveContinuation, maxRescopes, rescopeBudget, rescopeContinuation,
  maxReviewRepairs, previousReviewContinuation, previousReviewFindings, reviewRepairContinuation,
  prepareBuiltinPublication, runBuiltinAsk, runBuiltinIssue as runIssueWithSeats, stageReviewedFiles,
} from '../src/lib/builtin.mjs';
import { ToolAccessError } from '../src/runtime/tools.mjs';
import { loadLearning } from '../src/lib/learn.mjs';
import { readStatus, formatStatus } from '../src/lib/status.mjs';
import { resolveContractsPath } from '../src/lib/paths.mjs';
import { buildPublishMessage } from '../src/lib/publication.mjs';
import { parseRecipe } from '../src/lib/recipe.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { renderIssueBody } from '../src/lib/issue.mjs';
import { formatFleet } from '../src/lib/fleet.mjs';
import { LlmTimeoutError } from '../src/llm/request.mjs';
import { packAgentRun } from '../vendor/github-agent-contracts/scripts/parse-agent-run.mjs';
import { recordedCoderRun } from '../src/lib/seat-publication.mjs';
import { createDebugLog } from '../src/lib/debug-log.mjs';
import { readLocalRun } from '../src/lib/local-runs.mjs';
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';
import { createProvenanceStore } from '../src/lib/provenance-api.mjs';
import { createSteeringControl } from '../src/runtime/steering.mjs';
import {
  runBuiltinIssue, example, stubConfig, llmConfig, vllmConfig, multiFileScope, git, fixture, multiFileFixture,
} from './helpers/builtin.mjs';

test('builtin early failure and cancellation stay truthful after worktree cleanup without contacting a fleet', async (t) => {
  const options = fixture(t);
  const root = path.join(options.base, 'machine', 'provenance');
  const prior = createProvenanceStore({ root, repoRoot: options.target });
  await prior.recordEvent({ runId: 'prior-run', sessionId: 'prior-session', event: 'failure',
    payload: { outcome: 'failure', startedAt: '2025-01-01T00:00:00.000Z' } });
  const committed = (await openProvenanceStore(root).readAll()).records[0];
  const priorPath = path.join(root, 'log', `${committed.id}.json`);
  const bytes = readFileSync(priorPath);
  const failure = new Error('prepared handoff failed');
  await assert.rejects(runBuiltinIssue(42, { ...options, config: llmConfig, log: () => {},
    now: () => new Date('2025-01-01T00:00:01.000Z'),
    onPrepared: () => { throw failure; },
    fetchImpl: () => assert.fail('early failure must not contact the fleet'),
  }), (error) => error === failure);
  const afterFailure = await prior.query();
  const failedRun = afterFailure.find((record) => record.runId !== 'prior-run').runId;
  assert.deepEqual(afterFailure.filter((record) => record.runId === failedRun).map((record) => record.event),
    ['started', 'failure']);
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  git(options.target, 'worktree', 'remove', '--force', worktree);
  const controller = new AbortController();
  await assert.rejects(runBuiltinIssue(42, { ...options, config: llmConfig, log: () => {},
    now: () => new Date('2025-01-01T00:00:02.000Z'), signal: controller.signal,
    onPrepared: () => controller.abort(),
    fetchImpl: () => assert.fail('cancelled run must not contact the fleet'),
  }), { code: 'ROSTER_CANCELLED' });
  git(options.target, 'worktree', 'remove', '--force', worktree);
  const records = await createProvenanceStore({ root, repoRoot: options.target }).query();
  const cancelledRun = records.find((record) => !['prior-run', failedRun].includes(record.runId)).runId;
  assert.deepEqual(records.filter((record) => record.runId === cancelledRun).map((record) => record.event),
    ['started', 'cancellation']);
  assert.ok(!records.some((record) => record.event === 'completed'));
  assert.deepEqual(readFileSync(priorPath), bytes);
});

test('a repeated test failure escalates to fresh coder perspectives, then stops before review', async (context) => {
  const options = multiFileFixture(context);
  let tests = 0;
  let coderTurns = 0;
  let failed;
  const logs = [];
  const continuations = [];
  await assert.rejects(runBuiltinIssue(42, { ...options, config: llmConfig, log: (text) => logs.push(text),
    fetchImpl: async (_url, request) => {
      const system = JSON.parse(request.body).messages[0].content;
      if (system.startsWith('You are the builtin planner seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({ title: 'Add Status',
            acceptance_checks: ['node --test exits 0'], files_allowed: multiFileScope }),
        } }] });
      }
      assert.ok(!system.startsWith('You are the builtin reviewer seat.'), 'Stalled tests cannot request reviewer inference');
      coderTurns += 1;
      const fresh = system.match(/Fresh perspective (\d)/);
      if (fresh && !continuations.includes(fresh[1])) continuations.push(fresh[1]);
      return Response.json({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: 'Done.',
      } }] });
    },
    runTestCommand: async () => {
      tests += 1;
      assert.doesNotMatch(options.stderr, /Checking the diff/);
      throw Object.assign(new Error('tests failed'), { code: 1, stdout: 'not ok', stderr: 'assertion failed' });
    },
  }), (error) => {
    failed = error.result;
    return /Test repair stalled: an earlier failure repeated after 2 repairs/.test(error.message);
  });
  const attempts = 1 + maxPerspectiveEscalations;
  assert.equal(tests, 3 * attempts);
  assert.equal(coderTurns, 3 * attempts);
  assert.deepEqual(continuations, ['1', '2']);
  assert.equal(failed.repairRepeated, true);
  assert.equal(failed.review, undefined);
  assert.equal(existsSync(path.join(failed.resultPath, '..', 'REVIEW.md')), false);
  for (let attempt = 1; attempt <= 2; attempt += 1) {
    assert.match(options.stderr, new RegExp(`Tests failed\\. Repair ${attempt} of 4\\.`));
  }
  for (let attempt = 1; attempt <= maxPerspectiveEscalations; attempt += 1) {
    assert.match(logs.join('\n'), new RegExp(`Perspective escalation ${attempt} of ${maxPerspectiveEscalations}: ` +
      'coder repeated an earlier test failure; continuing with model=\\S+ in a fresh context\\.'));
  }
});

test('fresh perspectives carry earlier contexts and their failure counts, within the two-escalation cap', async (context) => {
  const options = multiFileFixture(context);
  let tests = 0;
  const logs = [];
  const systems = [];
  await assert.rejects(runBuiltinIssue(42, { ...options, config: llmConfig, log: (text) => logs.push(text),
    fetchImpl: async (_url, request) => {
      const system = JSON.parse(request.body).messages[0].content;
      if (system.startsWith('You are the builtin planner seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({ title: 'Add Status',
            acceptance_checks: ['node --test exits 0'], files_allowed: multiFileScope }),
        } }] });
      }
      systems.push(system);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }] });
    },
    runTestCommand: async () => {
      tests += 1;
      // Each context repeats its own failure three times; every fresh context has one fewer failing test.
      const failing = 10 - Math.floor((tests - 1) / 3);
      throw Object.assign(new Error('tests failed'), { code: 1, stdout: `not ok\nℹ fail ${failing}`, stderr: '' });
    },
  }), /Test repair stalled/);
  assert.equal(tests, 3 * (1 + maxPerspectiveEscalations));
  const text = logs.join('\n');
  assert.match(text, /Perspective escalation 1 of 2: coder repeated an earlier test failure with 10 failing test\(s\)/);
  assert.match(text, /Perspective escalation 2 of 2: coder repeated an earlier test failure with 9 failing test\(s\)/);
  assert.doesNotMatch(text, /Perspective escalation 3/);
  const last = systems.at(-1);
  assert.match(last, /Fresh perspective 2: .*Earlier contexts on this TASK: 1\) \S+ repeated an earlier test failure with 10 failing test\(s\); 2\) \S+ repeated an earlier test failure with 9 failing test\(s\)\. Do not retry their approaches/s);
  assert.match(last, /same model as the stopped context/);
});

test('a coder blocked only by the scope expansion limit is re-scoped instead of failing the run', async (context) => {
  const options = multiFileFixture(context);
  const config = { ...llmConfig, seat: { ...llmConfig.seat, scope_expansion: 1 } };
  const worktree = path.join(options.target, '.worktrees', 'issue-42');
  const logs = [];
  const events = [];
  let attempt = 0;
  let step = 0;
  const result = await runBuiltinIssue(42, { ...options, config, log: (text) => logs.push(text),
    onRunEvent: (event) => events.push(event),
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      const system = body.messages[0].content;
      if (system.startsWith('You are the builtin planner seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({ title: 'Add Status',
            acceptance_checks: ['node --test exits 0'], files_allowed: multiFileScope }),
        } }] });
      }
      if (system.startsWith('You are the builtin reviewer seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: passingReview(body, {
            reasons: ['Expanded files are justified.'], security_notes: ['No protected paths changed.'] }),
        } }] });
      }
      const rescoped = body.messages.some(({ content }) => /Re-scoped: .*1-file expansion budget/s.test(content ?? ''));
      if (rescoped && attempt === 0) { attempt = 1; step = 0; }
      step += 1;
      const writes = attempt === 0
        ? [['README.md', '# Example\n\n## Status\nReady.\n'], ['src/a.mjs', 'export const a = 1;\n'],
          ['src/b.mjs', 'export const b = 1;\n']]
        : [['src/b.mjs', 'export const b = 1;\n']];
      const write = writes[step - 1];
      return Response.json({ choices: [{ finish_reason: write ? 'tool_calls' : 'stop', message: write ? {
        role: 'assistant', tool_calls: [{ id: `code-${attempt}-${step}`, type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({ path: write[0], content: write[1] }),
        } }],
      } : { role: 'assistant', content: 'Added Status and its helpers.' } }] });
    },
    runTestCommand: async () => {
      if (!existsSync(path.join(worktree, 'src', 'b.mjs'))) {
        throw Object.assign(new Error('tests failed'), { code: 1, stdout: 'not ok', stderr: 'src/b.mjs missing' });
      }
      return { stdout: 'pass', stderr: '' };
    },
  });
  assert.equal(attempt, 1);
  assert.equal(result.review.verdict, 'pass');
  assert.deepEqual(result.rescopes, [{ from: 1, to: 3, files: ['src/b.mjs'] }]);
  assert.deepEqual(result.perspectiveAttempts, []);
  assert.deepEqual(result.result.scopeFiles, ['src/a.mjs', 'src/b.mjs']);
  assert.match(logs.join('\n'), /Re-scope 1 of 2: coder needed src\/b\.mjs beyond its 1-file expansion budget; continuing with 3 files/);
  assert.deepEqual(events.filter(({ type }) => type === 'rescope').map(({ from, to }) => [from, to]), [[1, 3]]);
});

test('a semantic review failure loops back to the coder, flags a stalled repair, and stops at its bound', async (context) => {
  const options = multiFileFixture(context);
  const logs = [];
  const events = [];
  const repairs = [];
  const reviewedPrevious = [];
  let reviews = 0;
  const result = await runIssueWithSeats(42, { ...options, config: llmConfig, log: (text) => logs.push(text),
    onRunEvent: (event) => events.push(event),
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      const system = body.messages[0].content;
      if (system.startsWith('You are the builtin planner seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({ title: 'Add Status',
            acceptance_checks: ['node --test exits 0', 'src/a.mjs exports ready'], files_allowed: multiFileScope }),
        } }] });
      }
      if (system.startsWith('You are the builtin reviewer seat.')) {
        reviews += 1;
        reviewedPrevious.push(/## Previous review findings\n\n- Check 2 unmet: src\/a\.mjs has no ready export\./
          .test(body.messages[1].content));
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify({
          verdict: 'fail', reasons: ['The ready export is missing.'], security_notes: [],
          checks: reviewedChecks(body).map((id) => ({ id, met: id !== 2,
            evidence: id === 2 ? 'src/a.mjs has no ready export.' : 'Tests exit 0.' })),
        }) } }] });
      }
      const text = JSON.stringify(body.messages);
      const repair = text.match(/Review repair (\d)/)?.[1];
      if (repair && !repairs.some(([round]) => round === repair)) {
        repairs.push([repair, /do not repeat that approach/.test(text), /Check 2 unmet: src\/a\.mjs has no ready export/.test(text)]);
      }
      return Response.json({ choices: [body.tools?.length && !text.includes('"role":"tool"') ? {
        finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{ id: `w-${reviews}`, type: 'function',
          function: { name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: `# Example\n\n## Status\n${reviews}.\n` }) } }] },
      } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Updated src/a.mjs.' } }] });
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.equal(result.review.verdict, 'fail');
  assert.equal(reviews, 1 + maxReviewRepairs);
  assert.deepEqual(reviewedPrevious, [false, true, true], 're-reviews judge the previous findings');
  assert.deepEqual(repairs, [['1', false, true], ['2', true, true]]);
  assert.deepEqual(result.reviewRepairs.map(({ unmetChecks }) => unmetChecks), [[2], [2]]);
  assert.match(logs.join('\n'), /Review repair 1 of 2: reviewer failed checks 2; continuing with model=\S+ in a fresh coder context/);
  assert.deepEqual(events.filter(({ type }) => type === 'review-repair').map(({ attempt }) => attempt), [1, 2]);
  assert.match(reviewRepairContinuation({ round: 1, reasons: ['Check 1 unmet: x'], unmetChecks: [1] }),
    /^Review repair 1: .*unmet acceptance checks: 1[\s\S]*- Check 1 unmet: x$/);
  assert.match(reviewRepairContinuation({ round: 1, reasons: ['x'] }),
    /RESULT\.md and REVIEW\.md are harness-written and read-only to you/);
});

test('a rerun of the same TASK carries the previous failed review findings, not a passing one', () => {
  const failed = '# Review\n\nVerdict: fail\n\n## Reasons\n\n- Check 2 unmet: tests/paths.test.mjs missing.\n- Wrong key.\n\n' +
    '## Acceptance checks\n\n- [x] 1. tests pass — ok\n- [ ] 2. table tests — missing\n\n## Security notes\n\n- None.\n';
  const carried = previousReviewContinuation(failed);
  assert.match(carried, /^Previous run: the reviewer failed the last result for this same TASK \(unmet acceptance checks: 2\)/);
  assert.match(carried, /- Check 2 unmet: tests\/paths\.test\.mjs missing\.\n- Wrong key\.$/);
  assert.doesNotMatch(carried, /Security notes|None\./);
  assert.equal(previousReviewContinuation(failed.replace('Verdict: fail', 'Verdict: pass')), undefined);
  assert.equal(previousReviewContinuation('# Review\n\nVerdict: fail\n\n## Reasons\n\n' +
    '- Reviewer could not complete: stdout maxBuffer length exceeded\n\n## Security notes\n\n- None.\n'), undefined);
  assert.equal(previousReviewContinuation(null), undefined);
  assert.deepEqual(previousReviewFindings(failed),
    { reasons: ['Check 2 unmet: tests/paths.test.mjs missing.', 'Wrong key.'], unmetChecks: [2] });
  assert.equal(previousReviewFindings(failed.replace('Verdict: fail', 'Verdict: pass')), undefined);
});

test('an incomplete reviewer gives no findings, so it does not restart the coder', async (context) => {
  const options = multiFileFixture(context);
  let reviews = 0;
  let coderTurns = 0;
  const result = await runIssueWithSeats(42, { ...options, config: llmConfig, log: () => {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      const system = body.messages[0].content;
      if (system.startsWith('You are the builtin planner seat.')) {
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify({ title: 'Add Status',
            acceptance_checks: ['node --test exits 0'], files_allowed: multiFileScope }),
        } }] });
      }
      if (system.startsWith('You are the builtin reviewer seat.')) {
        reviews += 1;
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content: 'not json' } }] });
      }
      coderTurns += 1;
      return Response.json({ choices: [coderTurns === 1 ? {
        finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{ id: 'w', type: 'function',
          function: { name: 'write_file', arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }) } }] },
      } : { finish_reason: 'stop', message: { role: 'assistant', content: 'Done.' } }] });
    },
    runTestCommand: async () => ({ stdout: 'pass', stderr: '' }),
  });
  assert.equal(result.review.verdict, 'fail');
  assert.match(result.review.reasons[0], /^Reviewer could not complete/);
  assert.equal(reviews, 2);
  assert.equal(coderTurns, 2);
  assert.deepEqual(result.reviewRepairs, []);
});

test('re-scoping respects strict scope, the 16-file ceiling, and its attempt limit', () => {
  const blocked = { result: { scopeBlocked: ['src/x.mjs'] } };
  assert.equal(rescopeBudget(blocked, 3, 0), 6);
  assert.equal(rescopeBudget({ result: { scopeBlocked: ['a', 'b', 'c', 'd', 'e', 'f'] } }, 3, 0), 10);
  assert.equal(rescopeBudget(blocked, 12, 1), 16);
  assert.equal(rescopeBudget(blocked, 16, 0), null);
  assert.equal(rescopeBudget(blocked, 0, 0), null);
  assert.equal(rescopeBudget(blocked, 3, maxRescopes), null);
  assert.equal(rescopeBudget({ result: { scopeBlocked: [] } }, 3, 0), null);
  assert.equal(rescopeBudget(new Error('x'), 3, 0), null);
  assert.match(rescopeContinuation({ previous: 3, budget: 6, files: ['src/x.mjs'], changedFiles: ['README.md'] }),
    /^Re-scoped: .*3-file expansion budget and needed src\/x\.mjs\. The budget is now 6 files.*\(README\.md\)/s);
});

test('coder stuck detection escalates budget exhaustion but never security denials', () => {
  const withResult = (error) => Object.assign(error, { result: {} });
  assert.equal(coderStuckReason(withResult(new Error('x', { cause: new Error('Test repair budget (4) exhausted') }))),
    'exhausted its test repair budget');
  assert.equal(coderStuckReason(Object.assign(new Error('x'), { result: { repairBudgetExhausted: true } })),
    'exhausted its test repair budget');
  assert.equal(coderStuckReason(withResult(new Error('Coder turn budget (12) exhausted'))), 'exhausted its turn budget');
  assert.equal(coderStuckReason(withResult(new Error('x', {
    cause: new ToolAccessError('Scope expansion limit reached (repeated after 2 denials)') }))), 'repeated a denied action');
  assert.equal(coderStuckReason(withResult(new Error('Test repair budget (4) exhausted', {
    cause: new ToolAccessError('Write denied: .github/workflows/ci.yml') }))), null);
  assert.equal(coderStuckReason(new Error('Test repair budget (4) exhausted')), null);
  assert.equal(coderStuckReason(withResult(new Error('Cancelled'))), null);
  assert.equal(coderStuckReason(Object.assign(new Error('x'), { result: { repairRepeated: true } })),
    'repeated an earlier test failure');
  assert.equal(coderStuckReason(withResult(new Error('Test repair stalled: an earlier failure repeated after 2 repairs'))),
    'repeated an earlier test failure');
  assert.equal(coderStuckReason(Object.assign(new Error('x'), { result: { contextHandoff: true } })),
    'filled half its context while repairs were still progressing');
  assert.equal(coderStuckReason(Object.assign(new Error('Coder excellence gate failed: Test substance: x'),
    { result: { substanceUnresolved: true } })), 'left a new test that does not exercise app code after its correction');
  const text = perspectiveContinuation({ attempt: 1, reason: 'exhausted its test repair budget',
    evidence: 'x'.repeat(5000), changedFiles: ['src/a.mjs'] });
  assert.match(text, /^Fresh perspective 1: .*\(src\/a\.mjs\).*never alternate between editing a test/s);
  assert.ok(text.length < 3000);
});
