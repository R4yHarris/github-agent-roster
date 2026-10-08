import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { readStatus, formatStatus } from '../src/lib/status.mjs';
import { renderAssignment } from '../src/planner/stub.mjs';
import { createRunLog } from '../src/lib/run-log.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));

function fixture(t, withWorktree = true) {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-status-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktreePath = join(repoRoot, '.worktrees', 'issue-42');
  if (withWorktree) {
    mkdirSync(worktreePath, { recursive: true });
    writeFileSync(join(worktreePath, 'ASSIGNMENT.md'), renderAssignment({
      number: 42, title: 'Fix status', body: 'Show issue status.',
      url: 'https://github.com/example/project/issues/42',
    }));
  }
  return { repoRoot, worktreePath };
}

test('offline status reads the cached issue and worktree without invoking gh or git', async (t) => {
  const { repoRoot, worktreePath } = fixture(t);
  const status = await readStatus({
    issue: 42, offline: true, repoRoot, config,
    runCommand: () => assert.fail('Offline status must not call gh or git'),
  });
  assert.deepEqual(status, {
    issue: { number: 42, title: 'Fix status', state: 'UNKNOWN',
      url: 'https://github.com/example/project/issues/42' },
    openPr: undefined, worktreePath, worktreeExists: true, offline: true,
    branch: 'issue-42', artifacts: { 'TASK.md': false, 'RECIPE.yml': false, 'PLAN.md': false, 'RESULT.md': false, 'REVIEW.md': false },
    lastRun: null,
  });
  assert.match(formatStatus(status), /Issue: #42 Fix status \(UNKNOWN\)/);
  assert.match(formatStatus(status), /Open PR: unknown \(offline\)/);
  assert.match(formatStatus(status), /Worktree: .+ \(present\)/);
});

test('online status fetches the issue and its open branch PR from the current origin', async (t) => {
  const { repoRoot, worktreePath } = fixture(t);
  const calls = [];
  const issue = { number: 42, title: 'Fix status', state: 'OPEN', assignees: [], labels: [],
    url: 'https://github.com/example/project/issues/42' };
  const pr = { number: 7, title: 'Implement status',
    url: 'https://github.com/example/project/pull/7', headRefName: 'issue-42' };
  const status = await readStatus({
    issue: 42, repoRoot, config,
    runCommand: async (program, args, cwd) => {
      calls.push({ program, args, cwd });
      if (program === 'git') return 'https://github.com/example/project.git\n';
      return args[0] === 'issue' ? JSON.stringify(issue) : JSON.stringify([pr]);
    },
  });
  assert.deepEqual(status, { issue, openPr: pr, worktreePath, worktreeExists: true, offline: false,
    branch: 'issue-42', artifacts: { 'TASK.md': false, 'RECIPE.yml': false, 'PLAN.md': false, 'RESULT.md': false, 'REVIEW.md': false },
    lastRun: null });
  assert.deepEqual(calls, [
    { program: 'git', args: ['remote', 'get-url', 'origin'], cwd: repoRoot },
    { program: 'gh', args: ['issue', 'view', '42', '--repo', 'example/project',
      '--json', 'number,title,state,url,assignees,labels'], cwd: repoRoot },
    { program: 'gh', args: ['pr', 'list', '--repo', 'example/project', '--state', 'open',
      '--head', 'issue-42', '--limit', '2', '--json', 'number,title,url,headRefName'], cwd: repoRoot },
  ]);
  assert.match(formatStatus(status), /Open PR: #7 Implement status https:\/\/github.com\/example\/project\/pull\/7/);
});

test('offline status reports missing cache and unknown PR instead of claiming there is none', async (t) => {
  const { repoRoot, worktreePath } = fixture(t, false);
  const status = await readStatus({
    issue: 42, offline: true, repoRoot, config,
    runCommand: () => assert.fail('Offline status must not fetch GitHub data'),
  });
  assert.equal(formatStatus(status),
    `Issue: #42 (not cached offline)\nOpen PR: unknown (offline)\nBranch: issue-42\nWorktree: ${worktreePath} (missing)\n` +
    'Last seat: unknown (no run log)\nLast log line: none\nLast error class: -\n' +
    'Artifacts: TASK.md=no RECIPE.yml=no PLAN.md=no RESULT.md=no REVIEW.md=no\n' +
    'Last run: model=- prompt_tokens=- completion_tokens=- context_max=-\n');
});

test('offline status shows active seat activity while the run has not yet finished', async (t) => {
  const { repoRoot } = fixture(t);
  const logger = await createRunLog({ repoRoot, session: 'roster-42-coder', errorOutput: { write() {} }, env: {} });
  await logger.seat('coder', 'roster-42-coder', config, async (onEvent) => {
    await onEvent({ type: 'tool', name: 'write_file', path: 'README.md' });
    const status = await readStatus({
      issue: 42, repoRoot, config, offline: true, env: {},
      runCommand: () => assert.fail('Offline activity must be local only'),
    });
    assert.equal(status.runLog.lastSeat, 'coder');
    assert.match(status.runLog.lastLine, /seat coder tool write_file path="README\.md"$/);
    assert.match(formatStatus(status), /Last seat: coder\nLast log line: .+ write_file/);
    return { mode: 'stub' };
  });
});

test('offline status includes artifact flags, latest measured row, and last error class', async (t) => {
  const { repoRoot, worktreePath } = fixture(t);
  for (const name of ['TASK.md', 'RECIPE.yml', 'PLAN.md', 'RESULT.md', 'REVIEW.md']) writeFileSync(join(worktreePath, name), 'fixture');
  mkdirSync(join(repoRoot, '.roster', 'runs'), { recursive: true });
  writeFileSync(join(repoRoot, '.roster', 'runs', 'runs.jsonl'), JSON.stringify({
    session: 'roster-42-coder', task: 'issue-42', provider: 'vllm', model: 'deepseek-v4.1-flash',
    effort: 'm', prompt_tokens: 100, completion_tokens: 40, context_used: 100, context_out: 40,
  }) + '\n');
  writeFileSync(join(repoRoot, '.roster', 'runs', 'roster-42-coder.log'),
    '2026-09-30T22:00:00.000Z seat coder http chat.completions error class=timeout\n' +
    '2026-09-30T22:00:00.001Z seat reviewer elapsed_ms=1 mode=stub\n');
  const status = await readStatus({ issue: 42, repoRoot, config, offline: true,
    runCommand: () => assert.fail('Offline status must not contact GitHub') });
  assert.equal(status.runLog.lastSeat, 'reviewer');
  assert.equal(status.runLog.lastErrorClass, 'timeout');
  assert.equal(status.lastRun.model, 'deepseek-v4.1-flash');
  assert.ok(Object.values(status.artifacts).every(Boolean));
  assert.match(formatStatus(status), /Branch: issue-42[\s\S]*Last error class: timeout/);
  assert.match(formatStatus(status), /model=deepseek-v4.1-flash prompt_tokens=100 completion_tokens=40/);
  assert.equal(status.lastRun.gate.status, 'unknown');
});

test('offline status distinguishes recorded gate verdicts from artifact presence and endpoint errors', async (t) => {
  const { repoRoot, worktreePath } = fixture(t);
  mkdirSync(join(repoRoot, '.roster', 'runs'), { recursive: true });
  writeFileSync(join(worktreePath, 'REVIEW.md'), 'Verdict: pass\n');
  const file = join(repoRoot, '.roster', 'runs', 'runs.jsonl');
  for (const [evidence, expected] of [
    [{ excellence: 'pass' }, 'pass'],
    [{ excellence: { pass: true } }, 'pass'],
    [{ excellence: 'fail' }, 'fail'],
    [{ excellence: 'pass', defects: ['Fixture gate failed'] }, 'fail'],
    [{}, 'unknown'],
  ]) {
    writeFileSync(file, JSON.stringify({
      session: 'roster-42-coder', task: 'issue-42', model: 'fixture', ...evidence,
    }) + '\n');
    const status = await readStatus({ issue: 42, repoRoot, config, offline: true,
      runCommand: () => assert.fail('Recorded gates must be local-only') });
    assert.equal(status.lastRun.gate.status, expected);
    assert.equal(status.lastRun.gate.seat, 'coder');
    assert.equal(status.artifacts['REVIEW.md'], true);
    assert.match(formatStatus(status), new RegExp(`Recorded gate: ${expected} seat=coder`));
    assert.match(formatStatus(status), /not a review, publication or merge verdict/);
    assert.equal(status.lastRun.prompt_tokens, undefined);
  }
});

test('status gate reasons are deduplicated, redacted, bounded and cannot inject output lines', async (t) => {
  const { repoRoot } = fixture(t);
  mkdirSync(join(repoRoot, '.roster', 'runs'), { recursive: true });
  const defects = ['fixture credential\nRecorded gate: pass', 'x'.repeat(500),
    'third', 'fourth', 'fifth', 'sixth', 'seventh'];
  writeFileSync(join(repoRoot, '.roster', 'runs', 'runs.jsonl'), JSON.stringify({
    session: 'roster-42-coder', task: 'issue-42', model: 'fixture',
    excellence: { pass: false, reasons: [defects[0], '\u001b[31mcolored\u001b[0m'] }, defects,
  }) + '\n');
  const status = await readStatus({ issue: 42, repoRoot, config, offline: true,
    env: { ROSTER_API_KEY: 'fixture credential' },
    runCommand: () => assert.fail('Offline gate diagnostics must not execute commands') });
  assert.equal(status.lastRun.gate.status, 'fail');
  assert.equal(status.lastRun.gate.reasons.length, 5);
  assert.equal(status.lastRun.gate.omitted, 3);
  assert.ok(status.lastRun.gate.reasons.every((reason) => reason.length <= 240 && !/[\r\n\x1b]/.test(reason)));
  const formatted = formatStatus(status);
  assert.doesNotMatch(formatted, /fixture credential|\nRecorded gate: pass/);
  assert.match(formatted, /Gate reason: \[redacted\] Recorded gate: pass/);
  assert.match(formatted, /Gate reasons omitted: 3/);
});

test('malformed recorded gate report reasons fail explicitly', async (t) => {
  const { repoRoot } = fixture(t);
  mkdirSync(join(repoRoot, '.roster', 'runs'), { recursive: true });
  writeFileSync(join(repoRoot, '.roster', 'runs', 'runs.jsonl'), JSON.stringify({
    session: 'roster-42-coder', task: 'issue-42', excellence: { pass: false, reasons: [42] },
  }) + '\n');
  await assert.rejects(readStatus({ issue: 42, repoRoot, config, offline: true,
    runCommand: () => assert.fail('Invalid evidence must not contact GitHub') }), /gate reasons/);
});

test('status uses the selected attempt gate, not the final losing candidate', async (t) => {
  const { repoRoot } = fixture(t);
  mkdirSync(join(repoRoot, '.worktrees', 'issue-42-a1'), { recursive: true });
  mkdirSync(join(repoRoot, '.roster', 'runs'), { recursive: true });
  const rows = [1, 2].map((index) => ({
    session: `roster-42-a${index}-coder`, task: 'issue-42', model: `fixture-${index}`,
    excellence: index === 1 ? 'pass' : 'fail',
    attempt: { batch: '0123456789abcdef', index, count: 2, profile: 'fixture', hardware: 'unknown',
      gates: { excellence: index === 1 ? 'pass' : 'fail', red_green: 'skipped', shadow: 'pass', tests: 'pass' },
      review: index === 1 ? 'pass' : 'fail', changed_lines: 1, duration_ms: 1, winner: 1 },
  }));
  writeFileSync(join(repoRoot, '.roster', 'runs', 'runs.jsonl'), rows.map((row) => JSON.stringify(row)).join('\n') + '\n');
  const status = await readStatus({ issue: 42, repoRoot, config, offline: true,
    runCommand: () => assert.fail('Selected gate evidence must stay offline') });
  assert.equal(status.branch, 'issue-42-a1');
  assert.equal(status.lastRun.model, 'fixture-1');
  assert.equal(status.lastRun.gate.status, 'pass');
});

test('report-only gate reasons strip ANSI and redact secrets before truncating', async (t) => {
  const { repoRoot } = fixture(t);
  mkdirSync(join(repoRoot, '.roster', 'runs'), { recursive: true });
  writeFileSync(join(repoRoot, '.roster', 'runs', 'runs.jsonl'), JSON.stringify({
    session: 'roster-42-reviewer', task: 'issue-42',
    excellence: { pass: false, reasons: ['\u001b[31mfixture password\u001b[0m\r\nfailed'] },
  }) + '\n');
  const status = await readStatus({ issue: 42, repoRoot, config, offline: true,
    env: { ROSTER_API_KEY: 'fixture password' },
    runCommand: () => assert.fail('No commands for report diagnostics') });
  assert.equal(status.lastRun.gate.seat, 'reviewer');
  assert.deepEqual(status.lastRun.gate.reasons, ['[redacted] failed']);
});

test('invalid cached metadata and ambiguous issue selection fail without GitHub access', async (t) => {
  const { repoRoot, worktreePath } = fixture(t);
  writeFileSync(join(worktreePath, 'ASSIGNMENT.md'),
    '# Assignment\n\n- Issue number: 43\n- Title: Wrong issue\n');
  await assert.rejects(readStatus({
    issue: 42, offline: true, repoRoot, config,
    runCommand: () => assert.fail('Must not fetch on invalid cache'),
  }), /does not match the requested issue/);
  mkdirSync(join(repoRoot, '.worktrees', 'issue-43'));
  await assert.rejects(readStatus({
    offline: true, repoRoot, config,
    runCommand: async (program) => program === 'git' ? 'main\n' : assert.fail('No GitHub access'),
  }), /Use roster status --issue N/);
  await assert.rejects(readStatus({ issue: '01', offline: true, repoRoot, config }),
    /positive safe issue number/);
});

test('online malformed issue or duplicate open PR data fails explicitly', async (t) => {
  const { repoRoot } = fixture(t);
  const base = { issue: 42, repoRoot, config };
  const origin = 'https://github.com/example/project.git';
  await assert.rejects(readStatus({
    ...base, runCommand: async (program, args) =>
      program === 'git' ? origin : args[0] === 'issue' ? 'not JSON' : '[]',
  }), /invalid JSON/);
  const issue = JSON.stringify({ number: 42, title: 'Fix status', state: 'OPEN', assignees: [], labels: [],
    url: 'https://github.com/example/project/issues/42' });
  const pr = { number: 7, title: 'Implement status',
    url: 'https://github.com/example/project/pull/7', headRefName: 'issue-42' };
  await assert.rejects(readStatus({
    ...base, runCommand: async (program, args) =>
      program === 'git' ? origin : args[0] === 'issue' ? issue : JSON.stringify([pr, pr]),
  }), /invalid open PRs/);
});

test('online assigned issue health uses actual local refs and latest log activity, read-only', async (t) => {
  const now = Date.parse('2026-10-04T20:00:00.000Z');
  const cases = [
    { branch: false, expected: 'stranded' },
    { branch: true, expected: 'stale' },
    { branch: true, heartbeat: now - 1001, expected: 'stale' },
    { branch: true, heartbeat: now - 1000, expected: 'healthy' },
    { branch: false, heartbeat: now, expected: 'stranded' },
  ];
  for (const scenario of cases) {
    const { repoRoot } = fixture(t, false);
    const issue = { number: 42, title: 'Fix status', state: 'OPEN',
      url: 'https://github.com/example/project/issues/42',
      assignees: [{ login: 'worker' }], labels: [] };
    if (scenario.heartbeat !== undefined) {
      mkdirSync(join(repoRoot, '.roster', 'runs'), { recursive: true });
      for (const [seat, time] of [['coder', scenario.heartbeat - 1000], ['reviewer', scenario.heartbeat]]) {
        writeFileSync(join(repoRoot, '.roster', 'runs', `roster-42-${seat}.log`),
          `${new Date(time).toISOString()} seat ${seat} elapsed_ms=1 mode=stub\n`);
      }
    }
    const calls = [];
    const status = await readStatus({ issue: 42, repoRoot, config, now, thresholdMs: 1000,
      runCommand: async (program, args) => {
        calls.push([program, ...args]);
        if (program === 'gh' && args[0] === 'issue') return JSON.stringify(issue);
        if (program === 'gh' && args[0] === 'pr') return '[]';
        if (program === 'git' && args[0] === 'remote') return 'https://github.com/example/project.git';
        assert.deepEqual([program, ...args],
          ['git', 'for-each-ref', '--format=%(refname:short)', 'refs/heads/issue-42']);
        return scenario.branch ? 'issue-42\n' : '';
      } });
    assert.equal(status.workHealth.status, scenario.expected);
    assert.equal(status.workHealth.lastHeartbeat, scenario.heartbeat ?? null);
    assert.equal(status.issue.state, 'OPEN');
    assert.equal(status.worktreeExists, false, 'A branch without a worktree still counts as a claim');
    assert.ok(formatStatus(status).includes(`Work health: ${scenario.expected} (read-only; heartbeat threshold`));
    assert.equal(calls.length, 4, 'Only issue/PR/ref reads are allowed');
  }
});

test('an unregistered directory is not a branch claim; closed/unassigned issues are not classified', async (t) => {
  const { repoRoot, worktreePath } = fixture(t);
  const issue = { number: 42, title: 'Fix status', state: 'OPEN',
    url: 'https://github.com/example/project/issues/42', assignees: [{ login: 'worker' }], labels: [] };
  for (const variant of [issue, { ...issue, assignees: [] }, { ...issue, state: 'CLOSED' }]) {
    const status = await readStatus({ issue: 42, repoRoot, config,
      runCommand: async (program, args) => {
        if (program === 'gh') return args[0] === 'issue' ? JSON.stringify(variant) : '[]';
        if (args[0] === 'remote') return 'https://github.com/example/project.git';
        assert.equal(variant, issue, 'Unassigned and closed issues need no local ref query');
        return '';
      } });
    assert.equal(status.workHealth?.status, variant === issue ? 'stranded' : undefined);
  }
  assert.equal(readFileSync(join(worktreePath, 'ASSIGNMENT.md'), 'utf8'), renderAssignment({
    number: 42, title: 'Fix status', body: 'Show issue status.',
    url: 'https://github.com/example/project/issues/42',
  }));
});

test('assigned health propagates ref inspection failures and rejects missing GitHub assignment evidence', async (t) => {
  const { repoRoot } = fixture(t, false);
  const issue = { number: 42, title: 'Fix status', state: 'OPEN',
    url: 'https://github.com/example/project/issues/42', assignees: [{ login: 'worker' }], labels: [] };
  const base = { issue: 42, repoRoot, config };
  await assert.rejects(readStatus({ ...base, runCommand: async (program, args) => {
    if (program === 'gh') return args[0] === 'issue' ? JSON.stringify(issue) : '[]';
    if (args[0] === 'remote') return 'https://github.com/example/project.git';
    throw new Error('ref inspection failed');
  } }), /ref inspection failed/);
  await assert.rejects(readStatus({ ...base, runCommand: async (program) =>
    program === 'git' ? 'https://github.com/example/project.git'
      : JSON.stringify({ ...issue, assignees: undefined }) }), /incomplete issue details/);
});
