import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { passingReview, reviewedChecks } from './helpers/review.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { previousFindingsSection, requirePassingReview, runReviewer, unchangedExistingFiles } from '../src/seats/reviewer.mjs';
import { LlmTimeoutError } from '../src/llm/request.mjs';
import { captureVerificationSubject } from '../src/runtime/independent-verification.mjs';

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

async function independentReceipt(options, actual = 'line 4') {
  const subject = await captureVerificationSubject({ worktree: options.worktree, files: ['src/app.mjs'] });
  return {
    schema_version: 1, subject_digest: subject.digest,
    observations: [1, 2].map((id) => ({
      id, check_id: id, expected_output: 'line 4', actual_output: actual,
      expected_exit: 1, actual_exit: 1, evidence: 'Executed original-Ask multiline diagnostic',
    })),
  };
}

function passingEndpoint(onRequest) {
  return async (url, request) => {
    onRequest?.();
    return { status: 200, json: async () => ({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: passingReview(JSON.parse(request.body)) } }],
      model: 'actual-review-model',
    }) };
  };
}

function dockerReviewOptions() {
  return { docker: { enabled: true, image: `sha256:${'a'.repeat(64)}`,
    plan: [1, 2].map((id) => ({ script: 'src/app.mjs', args: [], check_id: id,
      expected_stdout: 'line 4\n', expected_stderr: '', expected_exit: 0,
      evidence: `Host-authorized source diagnostic check ${id}` })) } };
}

function executableAuditFixture(context, extraAsk = '') {
  const options = fixture(context);
  const ask = `Export ready as true and provide a CLI that prints ready. Include Node built-in tests. ${extraAsk}` +
    'Output it in a new "src" directory.';
  writeFileSync(path.join(options.worktree, 'TASK.md'),
    '# Task: Executable audit\n\ndifficulty: 2\ntask_class: feat\n\n## Ask\n' + ask +
    '\n\n## Acceptance checks\n- node --test src exits 0\n- app exports ready\n\n## Files allowed\n' +
    '- src/app.mjs\n- src/cli.mjs\n- src/cli.test.mjs\n');
  writeFileSync(path.join(options.worktree, 'src', 'cli.mjs'),
    "import { ready } from './app.mjs';\nconsole.log(ready);\n");
  writeFileSync(path.join(options.worktree, 'src', 'cli.test.mjs'),
    "import test from 'node:test';\nimport assert from 'node:assert/strict';\n" +
    "import { execFileSync } from 'node:child_process';\n" +
    "test('CLI', () => assert.equal(execFileSync(process.execPath, ['src/cli.mjs'], " +
    "{ encoding: 'utf8' }).trim(), 'true'));\n");
  options.coderResult.excellence.files = ['src/app.mjs', 'src/cli.mjs', 'src/cli.test.mjs'];
  return options;
}

function auditReply(body, reason, ground) {
  return JSON.stringify({ verdict: 'fail', reasons: [reason], security_notes: [],
    checks: reviewedChecks(body).map(id => ({ id, met: false, evidence: reason })),
    grounds: [{ ...ground, reason }] });
}

test('executable Ask audit retries false import/dependency evidence against exact sources rather than accepting it', async (context) => {
  const options = executableAuditFixture(context);
  let calls = 0;
  const review = await runReviewer({ ...options, config,
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      calls++;
      let content = passingReview(body);
      if (calls === 1) {
        assert.match(body.messages[1].content, /## Exact delivered sources/);
        assert.match(body.messages[1].content, /import \{ ready \} from '\.\/app\.mjs'/);
        content = auditReply(body, 'CLI cannot resolve ../app.mjs without installing dependencies.', {
          obligation: 'provide a CLI that prints ready', file: 'src/cli.mjs',
          excerpt: "import { ready } from '../app.mjs';",
        });
      } else if (calls === 2) {
        assert.match(body.messages.at(-1).content, /exact Ask\/source-grounded/);
        assert.match(body.messages.at(-1).content, /An executed pass establishes only its recorded input/);
      }
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content } }] });
    },
  });
  assert.equal(calls, 3);
  assert.equal(review.verdict, 'pass', review.content);
  assert.equal(review.completed, true);
  assert.equal(execFileSync(process.execPath, [path.join(options.worktree, 'src', 'cli.mjs')],
    { cwd: tmpdir(), encoding: 'utf8' }).trim(), 'true');
});

test('persistently invented negative audit excerpts fail incomplete instead of receiving a passing fallback', async (context) => {
  const options = executableAuditFixture(context);
  let calls = 0;
  const review = await runReviewer({ ...options, config,
    fetchImpl: async (_url, request) => {
      calls++;
      const body = JSON.parse(request.body);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: auditReply(body, 'CLI requires an undeclared dependency.', {
          obligation: 'provide a CLI that prints ready', file: 'src/cli.mjs', excerpt: "import 'missing-package';",
        }) } }] });
    },
  });
  assert.equal(calls, 2);
  assert.equal(review.verdict, 'fail');
  assert.equal(review.completed, false);
  assert.match(review.content, /exact Ask\/source-grounded/);
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review }), /passing REVIEW/);
});

test('negative audit cannot introduce an extra working-directory obligation absent from the Ask', async (context) => {
  const options = executableAuditFixture(context);
  const review = await runReviewer({ ...options, config,
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: auditReply(body, 'Tests fail from inside src.', {
          obligation: 'Tests must also run from inside src.', file: 'src/cli.test.mjs',
          excerpt: "['src/cli.mjs']",
        }) } }] });
    },
  });
  assert.equal(review.verdict, 'fail');
  assert.equal(review.completed, false);
  assert.match(review.content, /exact Ask\/source-grounded/);
});

test('source-grounded requested cwd defect retains model veto despite passing covered host observations', async (context) => {
  const options = executableAuditFixture(context, 'Tests must also run from inside src. ');
  const env = { ...process.env };
  delete env.NODE_TEST_CONTEXT;
  const rootTests = execFileSync(process.execPath, ['--test', 'src'],
    { cwd: options.worktree, encoding: 'utf8', env });
  assert.match(rootTests, /# pass 1/);
  assert.throws(() => execFileSync(process.execPath, ['--test', 'cli.test.mjs'],
    { cwd: path.join(options.worktree, 'src'), encoding: 'utf8', stdio: 'pipe', env }),
  error => error.status === 1 && /# fail 1/.test(error.stdout) && /Cannot find module/.test(error.stdout));
  const subject = await captureVerificationSubject({ worktree: options.worktree,
    files: options.coderResult.excellence.files });
  const receipt = { ...(await independentReceipt(options)), subject_digest: subject.digest };
  let calls = 0;
  const review = await runReviewer({ ...options, config, independentVerification: { receipt },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      calls++;
      const content = calls === 1 ? auditReply(body, 'src/cli.test.mjs resolves src/cli.mjs inside src/src when run from src.', {
        obligation: 'Tests must also run from inside src.', file: 'src/cli.test.mjs', excerpt: "['src/cli.mjs']",
      }) : passingReview(body);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content } }] });
    },
  });
  assert.equal(calls, 2);
  assert.equal(review.completed, true);
  assert.equal(review.independentVerification.status, 'pass');
  assert.equal(review.verdict, 'fail');
  assert.match(review.content, /Original Ask audit.*src\/src/);
  assert.match(review.content, /### Negative finding grounds/);
  assert.match(review.content, /Ask: Tests must also run from inside src/);
  assert.match(review.content, /Source \(src\/cli.test.mjs\): \['src\/cli.mjs'\]/);
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review }), /passing REVIEW/);
});

test('negative audit source excerpts share redaction and whitespace rules and remain durable', async (context) => {
  const options = executableAuditFixture(context);
  writeFileSync(path.join(options.worktree, 'src', 'cli.mjs'),
    "import { ready } from './app.mjs';\nconsole.log(\n'grounding-test-private-key'\n);\n");
  const review = await runReviewer({ ...options, config, env: { OPENAI_API_KEY: 'grounding-test-private-key' },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      const audit = body.messages[0].content.includes('independent Original Ask audit');
      assert.doesNotMatch(JSON.stringify(body), /grounding-test-private-key/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: audit ? auditReply(body, 'src/cli.mjs prints a literal rather than the requested ready value.', {
          obligation: 'provide a CLI that prints ready', file: 'src/cli.mjs', excerpt: "console.log( '[redacted]' );",
        }) : passingReview(body) } }] });
    },
  });
  assert.equal(review.verdict, 'fail');
  assert.equal(review.completed, true, review.content);
  assert.match(review.content, /Source \(src\/cli.mjs\): console.log\( '\[redacted\]' \);/);
  assert.doesNotMatch(review.content, /grounding-test-private-key/);
});

test('negative audit may identify a deleted scoped deliverable but cannot mark an existing file absent', async (context) => {
  for (const deleted of [true, false]) {
    const options = executableAuditFixture(context);
    if (deleted) {
      options.git('add', 'src/cli.mjs');
      options.git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
        'commit', '-m', 'Existing CLI');
      rmSync(path.join(options.worktree, 'src', 'cli.mjs'));
    }
    const review = await runReviewer({ ...options, config,
      fetchImpl: async (_url, request) => {
        const body = JSON.parse(request.body);
        const audit = body.messages[0].content.includes('independent Original Ask audit');
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: audit ? auditReply(body, 'The requested CLI file is absent.', {
            obligation: 'provide a CLI that prints ready', file: 'src/cli.mjs', excerpt: null,
          }) : passingReview(body) } }] });
      },
    });
    assert.equal(review.verdict, 'fail');
    assert.equal(review.completed, deleted, review.content);
    if (deleted) assert.match(review.content, /Source \(src\/cli.mjs\): \[File absent\]/);
    else assert.match(review.content, /exact Ask\/source-grounded/);
  }
});

test('Docker execution is wired before model review and contradictions force actionable repair findings', async (context) => {
  const options = fixture(context);
  let executed = 0;
  let cleaned = 0;
  const review = await runReviewer({ ...options, config,
    independentVerification: dockerReviewOptions(),
    runVerificationCommand: async (program, args) => {
      if (args[0] === 'rm') { cleaned++; return {}; }
      if (args.includes('-e')) return { stdout: 'v20.20.2\n', stderr: '' };
      executed++;
      return { stdout: 'line 3\n', stderr: '' };
    },
    fetchImpl: passingEndpoint(() => assert.equal(executed, 2)),
  });
  assert.equal(review.verdict, 'fail', review.content);
  assert.equal(review.completed, true);
  assert.deepEqual(review.unmetChecks, [1, 2]);
  assert.equal(cleaned, 3);
  assert.equal(review.independentVerification.status, 'fail');
});

test('Docker infrastructure failure is incomplete without model approval and invalid inputs cannot enable execution', async (context) => {
  const options = fixture(context);
  let cleaned = 0;
  const review = await runReviewer({ ...options, config,
    independentVerification: dockerReviewOptions(),
    runVerificationCommand: async (program, args) => {
      if (args[0] === 'rm') { cleaned++; return {}; }
      throw Object.assign(new Error('Docker unavailable'), { code: 'ENOENT' });
    },
    fetchImpl: async () => assert.fail('Infrastructure failure must not ask a model to approve'),
  });
  assert.equal(review.verdict, 'fail');
  assert.equal(review.completed, false);
  assert.equal(review.independentVerification.status, 'incomplete');
  assert.match(review.content, /Docker unavailable/);
  assert.equal(cleaned, 1);
});

test('Docker checks reexecute against repaired bytes instead of reusing prior executed evidence', async (context) => {
  const options = fixture(context);
  let runs = 0;
  const settings = { ...options, config, independentVerification: dockerReviewOptions(),
    runVerificationCommand: async (program, args) => {
      if (args[0] === 'rm') return {};
      if (args.includes('-e')) return { stdout: 'v20.20.2\n', stderr: '' };
      runs++;
      return { stdout: readFileSync(options.source, 'utf8').includes('repaired') ? 'line 4\n' : 'line 3\n',
        stderr: '' };
    }, fetchImpl: passingEndpoint() };
  const first = await runReviewer(settings);
  assert.equal(first.verdict, 'fail');
  rmSync(first.reviewPath);
  writeFileSync(options.source, 'export const ready = true; // repaired\n');
  const repaired = await runReviewer(settings);
  assert.equal(repaired.verdict, 'pass', repaired.content);
  assert.notEqual(repaired.independentVerification.subjectDigest, first.independentVerification.subjectDigest);
  assert.equal(runs, 4);
  await requirePassingReview({ worktreePath: options.worktree, review: repaired });
});

test('invalid and missing Docker options fail closed and failed coders never execute Docker', async (context) => {
  for (const setting of [{ docker: undefined }, { docker: { ...dockerReviewOptions().docker, runCommand: () => {} } }]) {
    const options = fixture(context);
    const review = await runReviewer({ ...options, config, independentVerification: setting,
      runVerificationCommand: async () => assert.fail('Invalid options must not execute'),
      fetchImpl: async () => assert.fail('Invalid options must not invoke a model') });
    assert.equal(review.verdict, 'fail');
    assert.equal(review.completed, false);
  }
  const options = fixture(context);
  const review = await runReviewer({ ...options, config,
    coderResult: { ...options.coderResult, timedOut: true },
    independentVerification: dockerReviewOptions(),
    runVerificationCommand: async () => assert.fail('Failed coder must not execute Docker'),
    fetchImpl: async () => assert.fail('Failed coder must not invoke a model') });
  assert.equal(review.verdict, 'fail');
  assert.equal(review.completed, false);
});

test('opt-in real Docker reviewer integration rejects false approval and accepts corrected output', {
  skip: !process.env.ROSTER_VERIFICATION_TEST_IMAGE,
}, async (context) => {
  const options = fixture(context);
  const independentVerification = dockerReviewOptions();
  independentVerification.docker.image = process.env.ROSTER_VERIFICATION_TEST_IMAGE;
  writeFileSync(options.source, "console.log('line 3');\n");
  const settings = { ...options, config, independentVerification, fetchImpl: passingEndpoint() };
  const rejected = await runReviewer(settings);
  assert.equal(rejected.verdict, 'fail', rejected.content);
  assert.deepEqual(rejected.unmetChecks, [1, 2]);
  assert.equal(rejected.independentVerification.status, 'fail');
  rmSync(rejected.reviewPath);
  writeFileSync(options.source, "console.log('line 4');\n");
  const accepted = await runReviewer(settings);
  assert.equal(accepted.verdict, 'pass', accepted.content);
  assert.equal(accepted.independentVerification.status, 'pass');
  await requirePassingReview({ worktreePath: options.worktree, review: accepted });
  writeFileSync(options.source, "console.log('stale');\n");
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review: accepted }), /changed after/);
});

test('host-approved unchanged dependency is snapshotted and binds final publication', async (context) => {
  const options = fixture(context);
  const dependency = path.join(options.worktree, 'src', 'dependency.mjs');
  writeFileSync(dependency, 'export const value = 4;\n');
  const independentVerification = dockerReviewOptions();
  independentVerification.docker.additionalFiles = ['src/dependency.mjs', 'src\\app.mjs'];
  independentVerification.docker.verifiers = [{ name: 'oracle.mjs', source: 'host approved source' }];
  for (const entry of independentVerification.docker.plan) entry.script = 'verifier:oracle.mjs';
  const review = await runReviewer({ ...options, config, independentVerification,
    runVerificationCommand: async (command, args) => {
      if (args[0] === 'rm') return {};
      if (args.includes('-e')) return { stdout: 'v20.20.2\n', stderr: '' };
      const mount = args[args.indexOf('--mount') + 1];
      const snapshot = mount.slice('type=bind,source='.length, mount.indexOf(',target='));
      assert.equal(readFileSync(path.join(snapshot, 'src', 'dependency.mjs'), 'utf8'), 'export const value = 4;\n');
      return { stdout: 'line 4\n', stderr: '' };
    }, fetchImpl: passingEndpoint() });
  assert.equal(review.verdict, 'pass', review.content);
  assert.deepEqual(review.independentVerification.files, ['src/app.mjs', 'src/dependency.mjs']);
  await requirePassingReview({ worktreePath: options.worktree, review });
  writeFileSync(dependency, 'export const value = 3;\n');
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review }), /changed after review/);
});

test('additional dependency authority does not permit secret, escaping or malformed scopes', async (context) => {
  for (const additionalFiles of [
    ['.env'], ['../outside.mjs'], 'src/dependency.mjs', [null],
    Array.from({ length: 65 }, (_, index) => `src/dependency-${index}.mjs`),
  ]) {
    const options = fixture(context);
    const independentVerification = dockerReviewOptions();
    independentVerification.docker.additionalFiles = additionalFiles;
    const review = await runReviewer({ ...options, config, independentVerification,
      runVerificationCommand: async () => assert.fail('Invalid dependency must not execute'),
      fetchImpl: async () => assert.fail('Invalid dependency must not invoke a model') });
    assert.equal(review.verdict, 'fail');
    assert.equal(review.completed, false);
    assert.equal(review.independentVerification.status, 'incomplete');
  }
});

test('trusted host execution contradiction overrides reviewer pass and publication', async (context) => {
  const options = fixture(context);
  const receipt = await independentReceipt(options, 'line 3');
  const review = await runReviewer({ ...options, config, independentVerification: { receipt },
    fetchImpl: passingEndpoint() });
  assert.equal(review.verdict, 'fail', review.content);
  assert.equal(review.completed, true);
  assert.equal(review.independentVerification.status, 'fail');
  assert.deepEqual(review.unmetChecks, [1, 2]);
  assert.match(review.content, /Independent verification observation 1, check 1 failed/);
  assert.match(review.content, /## Independent verification\n\nStatus: fail/);
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review }), /passing REVIEW/);
});

test('main reviewer and independent Ask audit both receive exact redacted host execution facts', async (context) => {
  const options = fixture(context);
  const taskPath = path.join(options.worktree, 'TASK.md');
  writeFileSync(taskPath, readFileSync(taskPath, 'utf8')
    .replace('Update app.', 'Update app. Output it in a new "src" directory.'));
  const receipt = await independentReceipt(options);
  for (const observation of receipt.observations) {
    observation.expected_output = 'line 4 receipt-secret-value';
    observation.actual_output = 'line 4 receipt-secret-value';
    observation.evidence = 'Executed EOF counterexample traces endRecord through endField';
  }
  const requests = [];
  const review = await runReviewer({
    ...options, config, env: { OPENAI_API_KEY: 'receipt-secret-value' },
    independentVerification: { receipt },
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      const evidence = body.messages[1].content;
      assert.match(evidence, /## Independent executed observations/);
      assert.ok(evidence.includes(receipt.subject_digest));
      assert.match(evidence, /"expected_exit":1,"actual_exit":1/);
      assert.match(evidence, /Executed EOF counterexample traces endRecord through endField/);
      assert.match(evidence, /covers only the declared observations/);
      assert.match(evidence, /untrusted product data/);
      assert.doesNotMatch(evidence, /receipt-secret-value/);
      assert.match(evidence, /\[redacted\]/);
      assert.equal(body.tools, undefined);
      requests.push(body.messages[0].content);
      return Response.json({ model: 'actual-review-model', choices: [{
        finish_reason: 'stop', message: { role: 'assistant', content: passingReview(body) },
      }] });
    },
  });
  assert.equal(review.verdict, 'pass', review.content);
  assert.equal(requests.length, 2);
  assert.match(requests[0], /independent Original Ask audit/);
  assert.match(requests[1], /Return only JSON with verdict/);
  assert.doesNotMatch(requests[1], /Perform an independent Original Ask audit/);
});

test('required execution facts exceeding the remaining context budget fail without model invocation', async (context) => {
  const options = fixture(context);
  let baselineChars;
  const baseline = await runReviewer({
    ...options, config,
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      baselineChars = body.messages.reduce((sum, message) => sum + message.content.length, 0);
      return Response.json({ choices: [{ finish_reason: 'stop',
        message: { role: 'assistant', content: passingReview(body) } }] });
    },
  });
  assert.equal(baseline.verdict, 'pass');
  rmSync(baseline.reviewPath);
  const receipt = await independentReceipt(options);
  receipt.observations = Array.from({ length: 10 }, (_, index) => ({
    ...receipt.observations[index % 2], id: index + 1,
    expected_output: 'x'.repeat(1400), actual_output: 'x'.repeat(1400),
  }));
  const review = await runReviewer({
    ...options, config: { ...config, seat: { ...config.seat, context_chars: baselineChars + 10 } },
    independentVerification: { receipt },
    fetchImpl: async () => assert.fail('Required observations must not be silently dropped to fit'),
  });
  assert.equal(review.verdict, 'fail');
  assert.equal(review.completed, false);
  assert.match(review.content, /evidence exceeds seat.context_chars/);
});

test('passing host observations do not override a model-reported uncovered defect', async (context) => {
  const options = fixture(context);
  const receipt = await independentReceipt(options);
  const review = await runReviewer({
    ...options, config, independentVerification: { receipt },
    fetchImpl: async (_url, request) => Response.json({
      choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'fail', reasons: ['src/app.mjs has a separate uncovered defect.'],
          security_notes: [], checks: reviewedChecks(JSON.parse(request.body)) }) } }],
    }),
  });
  assert.equal(review.independentVerification.status, 'pass');
  assert.equal(review.verdict, 'fail');
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review }), /passing REVIEW/);
});

test('correct trusted host receipt preserves pass and publication refuses later product changes', async (context) => {
  const options = fixture(context);
  const receipt = await independentReceipt(options);
  const review = await runReviewer({ ...options, config, independentVerification: { receipt },
    fetchImpl: passingEndpoint() });
  assert.equal(review.verdict, 'pass', review.content);
  assert.equal(review.independentVerification.status, 'pass');
  await requirePassingReview({ worktreePath: options.worktree, review });
  writeFileSync(options.source, 'export const ready = false;\n');
  await assert.rejects(requirePassingReview({ worktreePath: options.worktree, review }),
    /Independent verification product bytes changed after review/);
});

test('missing required receipt fails incomplete before model inference', async (context) => {
  const options = fixture(context);
  const review = await runReviewer({ ...options, config, independentVerification: { receipt: undefined },
    fetchImpl: async () => assert.fail('Missing execution evidence must not invoke a model') });
  assert.equal(review.verdict, 'fail');
  assert.equal(review.completed, false);
  assert.equal(review.independentVerification.status, 'incomplete');
  assert.match(review.content, /receipt is missing, malformed, or bound to different product bytes/);
});

test('product modification during model review invalidates independent verification', async (context) => {
  const options = fixture(context);
  const receipt = await independentReceipt(options);
  const review = await runReviewer({ ...options, config, independentVerification: { receipt },
    fetchImpl: passingEndpoint(() => writeFileSync(options.source, 'export const ready = false;\n')) });
  assert.equal(review.verdict, 'fail');
  assert.equal(review.completed, false);
  assert.equal(review.independentVerification.status, 'incomplete');
  assert.match(review.content, /subject changed during review/);
});

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
  assert.match(seen[0][0].content, /TASK\.md Constraints only when they are consistent with the Original Ask/);
  assert.match(seen[0][0].content, /do not add unrelated conditions, execute forbidden actions/);
  assert.doesNotMatch(seen[0][0].content, /re-review/);
  assert.doesNotMatch(seen[0][1].content, /Previous review findings/);
  rmSync(path.join(options.worktree, 'REVIEW.md'));
  assert.equal((await reviewer(['Check 2 unmet: ready is not exported', ' '])).verdict, 'pass');
  assert.match(seen[1][0].content, /re-review after a repair[\s\S]*cite words from the check or TASK\.md/);
  assert.match(seen[1][1].content,
    /## RESULT\.md[\s\S]*## Previous review findings\n\n1\. Check 2 unmet: ready is not exported\n\n## Diff/);
});

for (const scenario of ['missing', 'duplicate', 'out-of-range', 'invalid-evidence', 'invalid-resolved', 'unresolved', 'resolved', 'corrected']) {
  test(`re-review requires explicit finding accounting: ${scenario}`, async (context) => {
    const options = fixture(context);
    let calls = 0;
    const result = await runReviewer({ ...options, config, env: {},
      previousFindings: ['Physical line differs from row index after a quoted newline', 'CRLF reproduces the same defect'],
      fetchImpl: async (_url, request) => {
        calls += 1;
        const body = JSON.parse(request.body);
        const findings = [
          { id: 1, resolved: scenario !== 'unresolved', evidence: 'src/app.mjs traces physical input lines.' },
          { id: 2, resolved: true, evidence: 'src/app.mjs counts CRLF as one line break.' },
        ];
        if (scenario === 'duplicate') findings[1].id = 1;
        if (scenario === 'out-of-range') findings[1].id = 3;
        if (scenario === 'invalid-evidence') findings[1].evidence = '';
        if (scenario === 'invalid-resolved') findings[1].resolved = 'yes';
        const report = JSON.parse(passingReview(body));
        if (scenario === 'missing' || scenario === 'corrected' && calls === 1) delete report.findings;
        else report.findings = findings;
        if (scenario === 'corrected' && calls === 2) {
          assert.match(body.messages.at(-1).content, /exactly 2 \{id, resolved, evidence\}/);
        }
        return Response.json({ choices: [{ finish_reason: 'stop', message: {
          role: 'assistant', content: JSON.stringify(report),
        } }] });
      },
    });
    const invalid = !['unresolved', 'resolved', 'corrected'].includes(scenario);
    assert.equal(calls, invalid || scenario === 'corrected' ? 2 : 1);
    assert.equal(result.verdict, ['resolved', 'corrected'].includes(scenario) ? 'pass' : 'fail');
    assert.equal(result.completed, !invalid);
    if (scenario === 'unresolved') assert.match(result.content, /Finding 1 unresolved/);
    if (!invalid) assert.match(result.content, /## Previous findings/);
  });
}

test('previous finding evidence is not silently clipped and context overflow fails before model calls', async (context) => {
  const findings = Array.from({ length: 9 }, (_, i) => `Finding ${i}: ${'e'.repeat(490)}`);
  assert.match(previousFindingsSection(findings), /9\. Finding 8:/);
  assert.ok(previousFindingsSection(findings).length > 3000);
  const options = fixture(context);
  const result = await runReviewer({ ...options, env: {}, previousFindings: findings,
    config: { ...config, seat: { ...config.seat, context_chars: 2000 } },
    fetchImpl: () => assert.fail('Incomplete finding evidence must not reach the model'),
  });
  assert.equal(result.verdict, 'fail');
  assert.equal(result.completed, false);
  assert.match(result.content, /Reviewer evidence exceeds seat.context_chars/);
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
  assert.match(systemInstructions, /Fail app code that matches, hardcodes, or special-cases a test sentinel/);
  assert.match(systemInstructions, /built inside the test itself.*tautological/);
  assert.match(systemInstructions, /A test title is not evidence/);
  assert.match(systemInstructions, /read the result back through the app/);
  assert.match(systemInstructions, /never treat a marker as source code/);
  assert.match(systemInstructions, /do not call that input sentinel-free/);
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

test('unchanged base modules imported by the diff are listed so the reviewer does not call them missing', async (context) => {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-unchanged-'));
  context.after(() => rmSync(worktree, { recursive: true, force: true }));
  mkdirSync(path.join(worktree, 'src', 'lib'), { recursive: true });
  mkdirSync(path.join(worktree, 'tests'));
  writeFileSync(path.join(worktree, 'src', 'lib', 'identity.mjs'), 'export const id = 1;\n');
  writeFileSync(path.join(worktree, 'src', 'lib', 'api.mjs'), "import { id } from './identity.mjs';\nexport const api = id;\n");
  writeFileSync(path.join(worktree, 'tests', 'api.test.mjs'),
    "import { api } from '../src/lib/api.mjs';\nimport { gone } from '../src/lib/missing.mjs';\n");
  writeFileSync(path.join(worktree, 'src', 'lib', 'runs.mjs'), 'export const runs = [];\n');
  assert.deepEqual(await unchangedExistingFiles(worktree, ['src/lib/api.mjs', 'tests/api.test.mjs'],
    ['src/lib/api.mjs', 'src/lib/runs.mjs', 'tests/api.test.mjs']), ['src/lib/identity.mjs', 'src/lib/runs.mjs']);
});

test('reviewer evidence lists earlier-wave modules so a parallel re-implementation fails', async (context) => {
  const options = fixture(context);
  mkdirSync(path.join(options.worktree, 'src', 'lib'), { recursive: true });
  writeFileSync(path.join(options.worktree, 'src', 'lib', 'store.mjs'), 'export function openStore(root) {}\n');
  const seen = [];
  const review = await runReviewer({ ...options, config, env: {}, priorWaveFiles: ['src/lib/store.mjs'],
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      seen.push(body.messages);
      return { status: 200, json: async () => ({ choices: [{ finish_reason: 'stop', message: {
        role: 'assistant', content: passingReview(body) } }] }) };
    } });
  assert.equal(review.verdict, 'pass');
  assert.match(seen[0][1].content,
    /## Earlier waves delivered\n\nEarlier slices of this plan merged these modules\. Fail the review when the diff re-implements[\s\S]*- export function openStore\(root\)\n\n## Diff/);
});
