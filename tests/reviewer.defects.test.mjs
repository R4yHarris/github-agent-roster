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

test('reviewer asks for original-Ask fidelity even when a planner check and green tests agree on wrong semantics', async (context) => {
  const options = fixture(context);
  writeFileSync(path.join(options.worktree, 'TASK.md'), '# Task: Integer parity\n\n' +
    'difficulty: 2\ntask_class: feat\n\n## Acceptance checks\n- isEven(0) returns false\n\n' +
    '## Files allowed\n- `src/app.mjs`\n\n## Ask\n' +
    'Create isEven(value) for safe integers, including positive, negative and zero values.\n');
  writeFileSync(path.join(options.worktree, 'src', 'app.mjs'),
    'export function isEven(value) { return value !== 0 && value % 2 === 0; }\n');
  const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      assert.match(body.messages[0].content, /Original Ask is authoritative/);
      assert.match(body.messages[0].content, /incorrect observable result.*mathematical invariant/);
      assert.match(body.messages[1].content, /isEven\(0\) returns false/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
          checks: [{ id: 1, met: true, evidence: 'src/app.mjs explicitly returns false for zero.' }],
          defects: [{ file: 'src/app.mjs', symbol: 'isEven', input: 'isEven(0)',
            outcome: 'returns false although zero is even; planner check contradicts the requested parity operation' }],
        }),
      } }] });
    },
  });
  assert.equal(result.verdict, 'fail');
  assert.match(result.content, /zero is even/);
});

test('reviewer preserves required written setup instructions without demanding deployment execution', async (context) => {
  const options = fixture(context);
  writeFileSync(path.join(options.worktree, 'TASK.md'), '# Task: Marketing setup\n\n' +
    'difficulty: 1\ntask_class: docs\n\n## Acceptance checks\n- Guide explains localhost preview\n\n' +
    '## Files allowed\n- `src/app.mjs`\n\n## Ask\n' +
    'Write setup instructions for obtaining and serving a public web page to market a business. Do not deploy it.\n');
  const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      const body = JSON.parse(request.body);
      assert.match(body.messages[0].content, /written setup.*not permission to execute/);
      assert.match(body.messages[0].content, /local-only preview.*public-serving instructions/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'fail', reasons: [
          'Original Ask requires public-serving instructions; localhost preview alone cannot market the business.',
        ], security_notes: [], checks: [{ id: 1, met: true, evidence: 'Preview steps exist.' }] }),
      } }] });
    },
  });
  assert.equal(result.verdict, 'fail');
  assert.match(result.content, /public-serving instructions/);
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

function directoryFixture(context) {
  const options = fixture(context);
  mkdirSync(path.join(options.worktree, 'new-design'));
  writeFileSync(path.join(options.worktree, 'new-design', 'guide.md'), '# Preview\nUse localhost for local preview.\n');
  options.coderResult.excellence.files = ['new-design/guide.md'];
  writeFileSync(path.join(options.worktree, 'TASK.md'), '# Task: Marketing setup\n\n' +
    'difficulty: 1\ntask_class: docs\n\n## Acceptance checks\n- The guide explains local preview\n\n' +
    '## Files allowed\n- `new-design/guide.md`\n\n## Ask\n' +
    'Write setup instructions for obtaining and serving a public web page to market a business. ' +
    'Output it in a new "new-design" directory\n');
  return options;
}

test('directory Ask audit is independent of planner checks and a failed audit overrides checklist pass', async (context) => {
  const options = directoryFixture(context);
  let requests = 0;
  const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      requests += 1;
      const body = JSON.parse(request.body);
      if (requests === 1) {
        assert.match(body.messages[0].content, /independent Original Ask audit/);
        assert.doesNotMatch(body.messages[1].content, /The guide explains local preview/);
        assert.match(body.messages[1].content, /serving a public web page/);
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
          content: JSON.stringify({ verdict: 'fail', reasons: ['Public hosting steps required by the Ask are absent.'],
            security_notes: [], checks: [{ id: 1, met: false, evidence: 'new-design/guide.md supplies no public hosting steps.' }] }),
        } }] });
      }
      assert.match(body.messages[1].content, /The guide explains local preview/);
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
          checks: [{ id: 1, met: true, evidence: 'Local preview is documented.' }] }),
      } }] });
    },
  });
  assert.equal(requests, 2);
  assert.equal(result.verdict, 'fail');
  assert.match(result.content, /Original Ask audit.*Public hosting/);
});

for (const scenario of ['pass', 'corrected', 'invalid', 'unknown-usage']) {
  test(`directory Ask audit handles ${scenario} with bounded calls and measured usage`, async (context) => {
    const options = directoryFixture(context);
    let requests = 0;
    const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
      fetchImpl: async (_url, request) => {
        requests += 1;
        const body = JSON.parse(request.body);
        assert.equal(body.tools, undefined);
        const isAudit = body.messages[0].content.includes('independent Original Ask audit');
        if (isAudit) {
          assert.match(body.messages[0].content, /Markdown design and instructions are the authorized/);
          assert.match(body.messages[0].content, /not merely ask the reader/);
        }

        const invalid = scenario === 'invalid' || scenario === 'corrected' && requests === 1;
        const content = invalid ? '{}' : JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
          checks: [{ id: 1, met: true, evidence: 'new-design/guide.md fulfills the requested written procedure.' }] });
        if (scenario === 'corrected' && requests === 2) {
          assert.match(body.messages.at(-1).content, /Invalid audit JSON/);
        }
        return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant', content } }],
          model: 'actual-review-model', ...(scenario === 'unknown-usage' && requests === 1 ? {} :
            { usage: { prompt_tokens: 7, completion_tokens: 2 } }) });
      },
    });
    assert.equal(requests, scenario === 'corrected' ? 3 : 2);
    assert.equal(result.verdict, scenario === 'invalid' ? 'fail' : 'pass', result.content);
    assert.equal(result.completed, scenario !== 'invalid');
    if (scenario !== 'invalid') assert.match(result.content, /## Original Ask audit\n\nVerdict: pass/);
    if (scenario === 'invalid') assert.match(result.content, /Reviewer could not complete/);
    else assert.deepEqual(result.usage, scenario === 'unknown-usage' ? {} :
      { prompt_tokens: requests * 7, completion_tokens: requests * 2 });
  });
}

test('directory audit sees unchanged allowed deliverables instead of treating a small diff as the whole output', async (context) => {
  const options = directoryFixture(context);
  execFileSync('git', ['add', 'new-design/guide.md'], { cwd: options.worktree });
  execFileSync('git', ['-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '-q', '-m', 'Existing guide'], { cwd: options.worktree });
  writeFileSync(path.join(options.worktree, 'new-design', 'note.md'), '# Additional note\n');
  const taskPath = path.join(options.worktree, 'TASK.md');
  writeFileSync(taskPath, readFileSync(taskPath, 'utf8').replace('- `new-design/guide.md`',
    '- `new-design/guide.md`\n- `new-design/note.md`'));
  options.coderResult.excellence.files = ['new-design/note.md'];
  let requests = 0;
  const result = await runReviewer({ ...options, config, env: {}, askKind: 'slice',
    fetchImpl: async (_url, request) => {
      requests += 1;
      const body = JSON.parse(request.body);
      if (requests === 1) {
        assert.match(body.messages[1].content, /### new-design\/guide.md\n\n# Preview\nUse localhost/);
      }
      return Response.json({ choices: [{ finish_reason: 'stop', message: { role: 'assistant',
        content: JSON.stringify({ verdict: 'pass', reasons: [], security_notes: [],
          checks: [{ id: 1, met: true, evidence: 'The unchanged guide is available for judgment.' }] }),
      } }] });
    },
  });
  assert.equal(requests, 2);
  assert.equal(result.verdict, 'pass', result.content);
});
