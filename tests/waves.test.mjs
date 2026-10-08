import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { renderPlan, validatePlan } from '../src/planner/plan.mjs';
import { parsePlanDocument } from '../src/planner/plan-document.mjs';
import { earlierWaveFiles, issueDependencies, requireEarlierWavesClosed, waveBoard } from '../src/lib/waves.mjs';
import { askRequirements, cleanAskText, renderAssignment } from '../src/planner/stub.mjs';
import { classifyAsk } from '../src/planner/classify.mjs';
import { formatHelp } from '../src/shell/commands.mjs';

function fixture(t) {
  const worktree = mkdtempSync(path.join(tmpdir(), 'roster-waves-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  const plan = validatePlan({ outcomes: ['Deliver a tested change'], issues: [
    { title: 'First slice', outcome: 'First outcome', acceptance_checks: ['node --test exits 0'], wave: 1, files_allowed: ['README.md'] },
    { title: 'Second slice', outcome: 'Second outcome', acceptance_checks: ['node --test exits 0'], wave: 2, files_allowed: ['README.md'] },
  ] }, { kind: 'feature', filesAllowed: ['README.md'] });
  const source = renderPlan(plan, { ask: 'Deliver two outcomes in README.md.', kind: 'feature', reference: 'local:wave-plan' });
  writeFileSync(path.join(worktree, 'PLAN.md'), source);
  const issues = [];
  const names = [];
  const calls = [];
  let review = false;
  const runCommand = async (program, args) => {
    calls.push([program, args]);
    if (program === 'git') return 'https://github.com/example/project.git\n';
    if (args[0] === 'label' && args[1] === 'list') return JSON.stringify(names.map((name) => ({ name })));
    if (args[0] === 'label' && args[1] === 'create') { names.push(args[2]); return ''; }
    if (args[0] === 'issue' && args[1] === 'list') return JSON.stringify(issues);
    if (args[0] === 'issue' && args[1] === 'create') {
      const issue = { number: 100 + issues.length, title: args[args.indexOf('--title') + 1], state: 'OPEN',
        body: args[args.indexOf('--body') + 1], labels: [{ name: args[args.indexOf('--label') + 1] }] };
      issues.push(issue);
      return `https://github.com/example/project/issues/${issue.number}\n`;
    }
    if (args[0] === 'pr') return JSON.stringify(review ? [{ number: 3 }] : []);
    throw new Error('Unexpected wave fixture command');
  };
  return { worktree, cwd: worktree, source, plan, issues, calls, runCommand, env: {}, setReview() { review = true; } };
}

test('waves are parsed from PLAN and default view creates no GitHub issues or board file', async (t) => {
  const options = fixture(t);
  assert.deepEqual(parsePlanDocument(options.source).issues, options.plan.issues);
  const rows = await waveBoard(options);
  assert.deepEqual(rows.map(({ wave, title, state }) => [wave, title, state]),
    [[1, 'First slice', 'todo'], [2, 'Second slice', 'todo']]);
  assert.equal(options.calls.some(([, args]) => args.includes('create')), false);
  assert.equal(readFileSync(path.join(options.worktree, 'PLAN.md'), 'utf8'), options.source);
  assert.match(formatHelp('waves'), /open: explicitly/);
});

test('explicit open creates only existing drafts, is idempotent and derives states from GitHub', async (t) => {
  const options = fixture(t);
  const opened = await waveBoard({ ...options, open: true });
  assert.equal(options.issues.length, 2);
  assert.equal(opened[1].state, 'blocked');
  await waveBoard({ ...options, open: true });
  assert.equal(options.issues.length, 2);
  options.issues[0].state = 'CLOSED';
  assert.deepEqual((await waveBoard(options)).map(({ state }) => state), ['done', 'todo']);
  // Roster App labels are the shared board: another agent's claim is never offered as the next slice.
  for (const [label, state] of [['roster:in-progress', 'running'], ['roster:review', 'review'], ['roster:blocked', 'blocked']]) {
    options.issues[1].labels = [{ name: 'wave:2' }, { name: label }];
    assert.equal((await waveBoard(options))[1].state, state);
  }
  options.issues[1].labels = [{ name: 'wave:2' }];
  options.setReview();
  assert.equal((await waveBoard(options))[1].state, 'review');
  assert.equal((await waveBoard({ ...options, activeIssue: options.issues[1].number, activeState: 'drafting' }))[1].state, 'running');
  await assert.rejects(waveBoard({ ...options, open: true, env: { ROSTER_SEAT: 'coder' } }), /explicit human command/);
});

test('opened child issues are runnable slices linked to their parent issue', async (t) => {
  const options = fixture(t);
  writeFileSync(path.join(options.worktree, 'PLAN.md'), options.source.replace('local:wave-plan', 'issue:196'));
  await waveBoard({ ...options, open: true });
  assert.equal(options.issues.length, 2);
  for (const issue of options.issues) {
    const ask = cleanAskText(issue.body);
    assert.match(ask, /^Parent: #196$/m);
    assert.equal(classifyAsk(ask, { title: `feature: ${issue.title}` }).kind, 'slice');
    assert.deepEqual(askRequirements(ask).files, ['README.md']);
  }
});

test('a re-plan refuses to open a second wave set while the parent has open children from a lost plan', async (t) => {
  const options = fixture(t);
  writeFileSync(path.join(options.worktree, 'PLAN.md'), options.source.replace('local:wave-plan', 'issue:196'));
  await waveBoard({ ...options, open: true });
  writeFileSync(path.join(options.worktree, 'PLAN.md'),
    options.source.replace('local:wave-plan', 'issue:196').replace('First outcome', 'Re-planned outcome'));
  await assert.rejects(waveBoard({ ...options, open: true }), /already has open wave children from another plan \(#100, #101\)/);
  assert.equal(options.issues.length, 2);
});

test('a later wave refuses startup while an earlier label issue is open and fails closed on lookup errors', async () => {
  const issue = { body: '', labels: [{ name: 'wave:2' }] };
  await assert.rejects(requireEarlierWavesClosed({ issue, repository: 'example/project', cwd: process.cwd(),
    runCommand: async (_program, args) => {
      assert.equal(args[args.indexOf('--label') + 1], 'wave:1');
      return JSON.stringify([{ number: 100, title: 'Earlier issue' }]);
    } }), /Wave 2 is blocked while wave 1 issue #100 is open/);
  await assert.rejects(requireEarlierWavesClosed({ issue, repository: 'example/project', cwd: process.cwd(),
    runCommand: async () => { throw new Error('offline'); } }), /later wave is blocked/);
  await requireEarlierWavesClosed({ issue, repository: 'example/project', cwd: process.cwd(), runCommand: async () => '[]' });
  assert.match(renderAssignment({ number: 108, title: 'Wave task', body: 'Update README.md.',
    url: 'https://github.com/example/project/issues/108', labels: issue.labels }), /- Wave: 2/);
});

test('earlier-wave files come from closed same-plan slices and skip tests', async () => {
  const key = 'a'.repeat(64);
  const body = (wave) => `<!-- Roster-Plan: ${key} -->\n<!-- Roster-Wave: ${wave} -->`;
  const issue = { number: 253, body: body(2), labels: [{ name: 'wave:2' }] };
  const calls = [];
  const files = await earlierWaveFiles({ issue, repository: 'example/project', worktree: 'wt', cwd: 'root',
    runCommand: async (program, args, cwd) => {
      calls.push([program, cwd, args.find((arg) => arg.startsWith('--grep=')) ?? args[args.indexOf('--search') + 1]]);
      if (program === 'gh') return JSON.stringify([{ number: 251, body: body(1), labels: [] },
        { number: 254, body: body(3), labels: [] }, { number: 253, body: body(2), labels: [] }]);
      return 'src/lib/redaction.mjs\ntests/redaction.test.mjs\nsrc/lib/store.test.mjs\ndocs/X.md\n\nsrc/lib/schema.mjs\n';
    } });
  assert.deepEqual(files, ['src/lib/redaction.mjs', 'src/lib/schema.mjs']);
  assert.deepEqual(calls.map(([program, cwd]) => `${program}@${cwd}`), ['gh@root', 'git@wt']);
  assert.match(calls[0][2], new RegExp(`Roster-Plan: ${key}`));
  assert.match(calls[1][2], /#251\(\[\^0-9\]\|\$\)$/);
  assert.deepEqual(await earlierWaveFiles({ issue: { ...issue, body: body(1), labels: [] }, repository: 'x/y',
    worktree: 'wt', cwd: 'root', runCommand: async () => { throw new Error('not called'); } }), []);
  assert.deepEqual(await earlierWaveFiles({ issue, repository: 'x/y', worktree: 'wt', cwd: 'root',
    runCommand: async () => { throw new Error('offline'); } }), []);
});

test('explicit dependency links override wave labels and fail closed on invalid or unavailable metadata', async () => {
  const issue = { number: 200, body: 'Depends on: #100, #101', labels: [{ name: 'wave:2' }] };
  assert.deepEqual(issueDependencies(issue), [100, 101]);
  assert.equal(issueDependencies({ body: '' }), null);
  assert.deepEqual(issueDependencies({ body: 'Depends on: none' }), []);
  for (const body of ['Depends on: #200', 'Depends on: #100, #100', 'Depends on: #100 garbage',
    'Depends on: none\nDepends on: #100', 'Depends on: #999999999999999999']) {
    assert.throws(() => issueDependencies({ ...issue, body }), /dependency links/);
  }
  const options = { issue, repository: 'example/project', cwd: process.cwd(),
    runCommand: async (_program, args) => JSON.stringify({ number: Number(args[2]), state: 'CLOSED' }) };
  await requireEarlierWavesClosed(options);
  await assert.rejects(requireEarlierWavesClosed({ ...options,
    runCommand: async () => JSON.stringify({ number: 100, state: 'OPEN' }) }), /dependency #100 is open/);
  await assert.rejects(requireEarlierWavesClosed({ ...options,
    runCommand: async () => { throw new Error('offline'); } }), /status is unavailable/);
  await requireEarlierWavesClosed({ ...options, issue: { ...issue, body: 'Depends on: none' },
    runCommand: () => assert.fail('Explicit independence needs no earlier-label lookup') });
});

test('created children carry issue-link dependencies and explicit independence drives the board', async (t) => {
  const options = fixture(t);
  await waveBoard({ ...options, open: true });
  assert.match(options.issues[0].body, /^Depends on: none$/m);
  assert.match(options.issues[1].body, /^Depends on: #100$/m);
  options.issues[1].body = options.issues[1].body.replace('Depends on: #100', 'Depends on: none');
  assert.deepEqual((await waveBoard(options)).map(({ state }) => state), ['todo', 'todo']);
});

test('closed children do not require a live lookup of their external dependencies', async (t) => {
  const options = fixture(t);
  await waveBoard({ ...options, open: true });
  options.issues[0].state = 'CLOSED';
  options.issues[0].body = options.issues[0].body.replace('Depends on: none', 'Depends on: #900');
  const rows = await waveBoard(options);
  assert.deepEqual(rows.map(({ state }) => state), ['done', 'todo']);
});

test('child creation orders wave dependencies even when draft numbering is not wave ordered', async (t) => {
  const options = fixture(t);
  const plan = { ...options.plan, issues: [...options.plan.issues].reverse() };
  writeFileSync(path.join(options.worktree, 'PLAN.md'), renderPlan(plan, {
    ask: 'Deliver two outcomes in README.md.', kind: 'feature', reference: 'local:wave-plan',
  }));
  const rows = await waveBoard({ ...options, open: true });
  assert.match(options.issues[1].body, /^Depends on: #100$/m);
  assert.deepEqual(rows.map(({ issue, state }) => [issue, state]), [[101, 'blocked'], [100, 'todo']]);
});

test('closed explicit dependency files are reused even within the same labeled wave', async () => {
  const files = await earlierWaveFiles({ issue: { number: 101, body: 'Depends on: #100', labels: [{ name: 'wave:1' }] },
    repository: 'example/project', worktree: 'wt', cwd: 'root',
    runCommand: async (program, args) => {
      assert.equal(program, 'git');
      assert.ok(args.some((arg) => arg.includes('#100')));
      return 'src/lib/store.mjs\n';
    },
  });
  assert.deepEqual(files, ['src/lib/store.mjs']);
});
