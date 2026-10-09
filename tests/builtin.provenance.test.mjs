import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { openProvenanceStore } from '../src/lib/provenance-store.mjs';
import { storeRecordId } from '../src/lib/provenance-api.mjs';
import { runBuiltinIssue as runIssueWithSeats } from '../src/lib/builtin.mjs';
import { fixture, multiFileFixture, git, llmConfig, stubConfig, runBuiltinIssue } from './helpers/builtin.mjs';
import { passingReview } from './helpers/review.mjs';

test('seat provenance retains requested/served models, observed times, route and response-backed usage', async (t) => {
  const options = fixture(t);
  const date = new Date('2026-10-09T00:00:00.000Z');
  let calls = 0;
  const result = await runBuiltinIssue(42, { ...options,
    config: { ...llmConfig, llm: { ...llmConfig.llm, fleet_profile: 'fixture-route', hardware: 'fixture-gpu' } },
    now: () => date, log: () => {},
    fetchImpl: async () => Response.json({ model: 'served-alias',
      usage: { prompt_tokens: 0, completion_tokens: 7 },
      choices: [{ finish_reason: 'tool_calls', message: { role: 'assistant', tool_calls: [{
        id: `bad-${++calls}`, type: 'function', function: { name: 'write_file', arguments: 'garbage' },
      }] } }] }),
  });
  assert.equal(result.failed, true);
  const { records } = await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll();
  const planner = records.find((record) => record.event === 'session');
  assert.equal(planner.requestedModel, 'local-model');
  assert.equal(planner.servedModel, 'served-alias');
  assert.deepEqual(planner.route, { name: 'fixture-route', profile: 'fixture-route', hardware: 'fixture-gpu' });
  assert.equal(planner.startedAt, date.toISOString());
  assert.equal(planner.endedAt, date.toISOString());
  assert.equal(planner.metrics.tokens_prompt, 0);
  assert.equal(planner.metrics.tokens_completion, 7);
  assert.equal(planner.metrics.cost_usd, 'unknown');
  assert.ok(planner.metrics.duration_ms >= 0);
  assert.equal(planner.outcome, '', 'planning evidence must not invent success');
});

test('stub seat evidence leaves models and usage unknown rather than borrowing environment values', async (t) => {
  const options = fixture(t);
  await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {},
    env: { ...options.env, AI_CONTEXT_USED: '999', AI_CONTEXT_OUT: '888' },
  });
  const { records } = await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll();
  const planner = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-planner');
  assert.equal(planner.requestedModel, '');
  assert.equal(planner.servedModel, '');
  assert.equal(planner.metrics.tokens_prompt, 'unknown');
  assert.equal(planner.metrics.tokens_completion, 'unknown');
  const coder = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-coder');
  assert.deepEqual(coder.evidence.attempt, { sessionId: 'roster-42-coder', index: 1 });
  assert.equal(coder.id, storeRecordId(coder.repoIdentity, coder.runId, 'roster-42-coder', 'session', 'raw-history'));
  assert.equal(coder.evidence.verification.exit_code, undefined);
  const reviewer = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-reviewer');
  assert.equal(reviewer.evidence.review.verdict, 'fail');
  assert.equal(reviewer.evidence.review.completed, false);
  assert.equal(reviewer.evidence.review.queried, false);
  assert.ok(records.every((record) => record.evidence.eval === undefined));
});

async function configuredEvidence(t, { tests, incomplete = false, exception, docsOnly = false,
  infrastructureError = false, unknownTool, initialFailures = 0 } = {}) {
  const options = multiFileFixture(t);
  if (exception) options.issue.body = options.issue.body.replace('`smoke.test.mjs`', '`planned.test.mjs`');
  let coderTurns = 0;
  let testRuns = 0;
  const execute = () => runIssueWithSeats(42, { ...options, config: llmConfig, log: () => {},
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      const system = body.messages[0].content;
      let message;
      if (system.startsWith('You are the builtin planner seat.')) {
        message = { role: 'assistant', content: JSON.stringify({ title: 'Add Status',
          acceptance_checks: ['README has a Status section'], files_allowed: docsOnly ? ['README.md'] :
            ['README.md', exception ? 'planned.test.mjs' : 'smoke.test.mjs'] }) };
      } else if (system.startsWith('You are the builtin reviewer seat.')) {
        message = { role: 'assistant', content: incomplete ? 'not a review' : passingReview(body) };
      } else if (system.startsWith('You are the builtin research step.')) {
        message = { role: 'assistant', content: 'Inventory reviewed.' };
      } else {
        coderTurns++;
        message = unknownTool && coderTurns === 1 ? { role: 'assistant', tool_calls: [{
          id: 'unknown', type: 'function', function: { name: unknownTool, arguments: '{}' },
        }] } : coderTurns === (unknownTool ? 2 : 1) ? { role: 'assistant', tool_calls: [
          { id: 'read', type: 'function', function: { name: 'read_file',
            arguments: JSON.stringify({ path: 'README.md' }) } },
          { id: 'write', type: 'function', function: { name: 'write_file',
            arguments: JSON.stringify({ path: 'README.md', content: '# Example\n\n## Status\nReady.\n' }) } },
        ] } : { role: 'assistant', content: 'Added Status.' };
      }
      return Response.json({ model: 'served-model', choices: [{
        finish_reason: message.tool_calls ? 'tool_calls' : 'stop', message,
      }] });
    },
    runTestCommand: async (program, args, commandOptions) => {
      if (docsOnly) assert.fail('Docs-only task must not execute tests');
      if (infrastructureError) throw new Error('private-infrastructure-error-marker');
      if (program === 'git') return { stdout: git(commandOptions.cwd, ...args), stderr: '' };
      if (exception) {
        const full = args.includes('--test') && !args.includes('smoke.test.mjs');
        if (full || (exception === 'baseline' && args.includes('smoke.test.mjs'))) {
          throw Object.assign(new Error('outside failure'), {
            code: 1, stdout: 'test at smoke.test.mjs:1:1\n', stderr: '',
          });
        }
        return { stdout: 'pass', stderr: '' };
      }
      testRuns++;
      if (tests.exit_code !== 0 || testRuns <= initialFailures) throw Object.assign(new Error('tests failed'), {
        code: tests.exit_code || 1, stdout: 'test-output-private-marker', stderr: 'stderr-private-marker',
      });
      return { stdout: 'test-output-private-marker', stderr: 'stderr-private-marker' };
    },
  });
  let result;
  if (infrastructureError || tests.exit_code !== 0 && !exception) {
    await assert.rejects(execute, infrastructureError ? /node --test could not run/ : /Final node --test failed/);
  } else {
    result = await execute();
  }
  const { records } = await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll();
  return { result, records, options };
}

test('configured sessions persist actual verification and review artifact hash without output', async (t) => {
  const { result, records, options } = await configuredEvidence(t, { tests: {
    exit_code: 0,
  } });
  const coder = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-coder');
  assert.deepEqual(coder.evidence.verification, { exit_code: 0, skipped: false });
  const reviewer = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-reviewer');
  assert.equal(reviewer.evidence.review.verdict, 'pass');
  assert.equal(reviewer.evidence.review.completed, true);
  assert.deepEqual(reviewer.evidence.review.unmet_checks, []);
  assert.deepEqual(reviewer.evidence.review.artifact, { file: 'REVIEW.md',
    sha256: createHash('sha256').update(readFileSync(result.review.reviewPath)).digest('hex') });
  assert.doesNotMatch(JSON.stringify(records), /test-output-private-marker|stderr-private-marker|## Status/);
  git(options.target, 'worktree', 'remove', '--force', result.worktreePath);
  const recovered = await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll();
  assert.deepEqual(recovered.records, records, 'verification and review survive actual worktree removal');
});

test('failed coder persists its actual nonzero verification without inventing a review', async (t) => {
  const { records } = await configuredEvidence(t, { tests: {
    exit_code: 1,
  }, incomplete: true });
  const coder = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-coder');
  assert.equal(coder.evidence.verification.exit_code, 1);
  assert.equal(coder.outcome, 'fail');
  assert.equal(records.find((record) => record.event === 'session' && record.seat.name === 'reviewer'), undefined);
});

test('incomplete configured reviewer persists failure and actual artifact reference', async (t) => {
  const { records } = await configuredEvidence(t, { tests: { exit_code: 0 }, incomplete: true });
  const reviewer = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-reviewer');
  assert.equal(reviewer.evidence.review.verdict, 'fail');
  assert.equal(reviewer.evidence.review.completed, false);
  assert.equal(reviewer.evidence.review.queried, true);
});

test('documentation verification records an intentional skip without a test exit', async (t) => {
  const { records } = await configuredEvidence(t, { tests: { exit_code: 0 }, docsOnly: true });
  const coder = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-coder');
  assert.deepEqual(coder.evidence.verification, { skipped: true });
});

test('configured seats persist actual successful tool signals and numeric zero exits', async (t) => {
  const { records, options } = await configuredEvidence(t, { tests: { exit_code: 0 } });
  const coder = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-coder');
  assert.deepEqual(coder.evidence.observed_tool_events, {
    read_file: { ok: 1, error: 0, denied: 0 },
    write_file: { ok: 1, error: 0, denied: 0 },
    run_test: { ok: 1, error: 0, denied: 0, exit_zero: 1 },
  });
  assert.doesNotMatch(JSON.stringify(coder.evidence.observed_tool_events),
    /test-output-private-marker|stderr-private-marker|## Status|README\.md/);
  git(options.target, 'worktree', 'remove', '--force', path.join(options.target, '.worktrees', 'issue-42'));
  assert.deepEqual((await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll()).records,
    records, 'observed tool outcomes survive actual worktree removal');
});

test('stub coder and reviewer persist no invented tool signals', async (t) => {
  const options = fixture(t);
  await runBuiltinIssue(42, { ...options, config: stubConfig, log: () => {} });
  const { records } = await openProvenanceStore(path.join(options.base, 'machine', 'provenance')).readAll();
  for (const record of records.filter((record) => record.event === 'session' &&
    ['roster-42-coder', 'roster-42-reviewer'].includes(record.sessionId))) {
    assert.equal(record.evidence.observed_tool_events, undefined);
  }
});

test('tool infrastructure errors remain errors without invented numeric exits', async (t) => {
  const { records } = await configuredEvidence(t, { tests: { exit_code: 0 }, infrastructureError: true });
  const coder = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-coder');
  assert.deepEqual(coder.evidence.observed_tool_events.run_test, { ok: 0, error: 1, denied: 0 });
  assert.doesNotMatch(JSON.stringify(records), /private-infrastructure-error-marker/);
});

test('a repaired test preserves both nonzero and zero exit events in the same occurrence', async (t) => {
  const { records } = await configuredEvidence(t, { tests: { exit_code: 0 }, initialFailures: 1 });
  const coder = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-coder');
  const events = coder.evidence.observed_tool_events.run_test;
  assert.equal(events.exit_nonzero, 1);
  assert.ok(events.exit_zero >= 1);
  assert.equal(events.ok, events.exit_nonzero + events.exit_zero);
  assert.equal(coder.evidence.verification.exit_code, 0);
});

test('unrecognized tool names use one fixed bucket without persisting the label', async (t) => {
  const { records } = await configuredEvidence(t, { tests: { exit_code: 0 },
    unknownTool: 'private-unrecognized-tool-marker' });
  const coder = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-coder');
  assert.deepEqual(coder.evidence.observed_tool_events.unknown, { ok: 0, error: 0, denied: 1 });
  assert.equal(Object.keys(coder.evidence.observed_tool_events).length, 4);
  assert.ok(JSON.stringify(coder.evidence.observed_tool_events).length < 512);
  assert.doesNotMatch(JSON.stringify(records), /private-unrecognized-tool-marker/);
});

test('classified transient and baseline exceptions retain their actual verification evidence', async (t) => {
  for (const exception of ['transient', 'baseline']) {
    await t.test(exception, async (context) => {
      const { records } = await configuredEvidence(context, { tests: { exit_code: 1 }, exception });
      const coder = records.find((record) => record.event === 'session' && record.sessionId === 'roster-42-coder');
      const verification = coder.evidence.verification;
      assert.equal(verification.skipped, false);
      if (exception === 'transient') {
        assert.equal(verification.exit_code, 0);
        assert.equal(verification.full_suite_exit_code, 1);
        assert.deepEqual(verification.transient, { full_suite_exit_code: 1, rerun_passed: ['smoke.test.mjs'] });
      } else {
        assert.equal(verification.exit_code, 1);
        assert.deepEqual(verification.baseline, { preexisting_failures: 1, files: ['smoke.test.mjs'] });
      }
    });
  }
});
