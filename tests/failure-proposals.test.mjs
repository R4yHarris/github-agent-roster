import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync, linkSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { createDispatcher } from '../src/repl.mjs';
import { failureSignatures, formatFailureProposals, listFailureProposals, proposeRecurringFailures,
  renderFailureProposal } from '../src/lib/failure-proposals.mjs';

const failures = [41, 42, 43].map((number, index) => ({
  task: `issue-${number}`, session: `roster-${number}-coder`, model: index === 0 ? 'model-one' : 'model-two',
  excellence: 'fail', defects: [
    `Shadow module: src/new-${number}.mjs adds readThing${number}, duplicates src/old-${number}.mjs at line ${number}`,
  ],
}));

function fixture(t, records = failures, evaluations = []) {
  const cwd = mkdtempSync(path.join(tmpdir(), 'roster-proposals-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  mkdirSync(path.join(cwd, '.roster', 'runs'), { recursive: true });
  mkdirSync(path.join(cwd, 'skills'));
  mkdirSync(path.join(cwd, 'principals'));
  writeFileSync(path.join(cwd, 'skills', 'existing.md'), '# Existing skill\n');
  writeFileSync(path.join(cwd, 'principals', 'existing.md'), '# Existing principal\n');
  writeFileSync(path.join(cwd, '.roster', 'runs', 'runs.jsonl'), records.map((record) => JSON.stringify(record)).join('\n') + (records.length ? '\n' : ''));
  if (evaluations.length) writeFileSync(path.join(cwd, '.roster', 'evals.jsonl'),
    evaluations.map((record) => JSON.stringify(record)).join('\n') + '\n');
  return cwd;
}

test('three shadow failures across two models collapse to one concrete signature', () => {
  const signatures = failureSignatures(failures);
  assert.equal(signatures.length, 1);
  assert.equal(signatures[0].target, 'skill');
  assert.deepEqual(signatures[0].occurrences.map(({ issue }) => issue), ['41', '42', '43']);
  assert.equal(new Set(signatures[0].occurrences.map(({ model }) => model)).size, 2);
  assert.match(signatures[0].reason, /<path>/);
  assert.match(signatures[0].reason, /<identifier>/);
  assert.match(signatures[0].reason, /<number>/);
  assert.equal(failureSignatures([...failures, ...failures]).length, 1);
  assert.equal(failureSignatures([...failures, ...failures])[0].occurrences.length, 3);
  const separate = failureSignatures([...failures, { ...failures[0], defects: ['Not red: test already passes'] }]);
  assert.equal(separate.length, 2);
});

test('different paths, numbers and quoted exports normalize, but distinct gates do not', () => {
  const records = [
    { ...failures[0], defects: ['Scope: `loadOne` in C:\\repo\\src\\one.mjs needs 4 files'] },
    { ...failures[1], defects: ['Scope: `loadTwo` in src/two.mjs needs 9 files'] },
  ];
  const groups = failureSignatures(records);
  assert.equal(groups.length, 1);
  assert.equal(groups[0].target, 'principal');
  assert.equal(groups[0].occurrences.length, 2);
  assert.throws(() => failureSignatures([null]), /object records/);
});

test('real shadow-gate reasons normalize simple export names as well as camel-case symbols', () => {
  const records = ['alpha', 'beta', 'gamma'].map((name, index) => ({
    ...failures[index],
    defects: [`Shadow module: src/${name}.mjs adds ${name}, which src/previous.mjs already exports; ` +
      `import or extend ${name} in src/previous.mjs instead.`],
  }));
  assert.equal(failureSignatures(records).length, 1);
  assert.equal(failureSignatures(records)[0].occurrences.length, 3);
  const otherCategory = { ...failures[0], defects: [
    'Shadow module: src/other.mjs exports delta, but no product module uses it (only tests, or nothing)',
  ] };
  assert.equal(failureSignatures([...records, otherCategory]).length, 2);
});

test('proposals cite all failures, preserve human edits, and never alter active rules', async (t) => {
  const cwd = fixture(t);
  const result = await proposeRecurringFailures({ cwd, env: {} });
  assert.equal(result.created.length, 1);
  const file = path.join(cwd, '.roster', 'proposals', result.created[0]);
  const content = readFileSync(file, 'utf8');
  for (const number of [41, 42, 43]) assert.match(content, new RegExp(`Issue: #${number}`));
  assert.match(content, /model-one/);
  assert.match(content, /model-two/);
  assert.match(content, /human review required/);
  assert.match(content, /## Proposed rule/);
  writeFileSync(file, '# Human edited draft\n');
  const again = await proposeRecurringFailures({ cwd, env: {} });
  assert.deepEqual(again.created, []);
  assert.deepEqual(again.existing, result.created);
  assert.equal(readFileSync(file, 'utf8'), '# Human edited draft\n');
  assert.equal(readFileSync(path.join(cwd, 'skills', 'existing.md'), 'utf8'), '# Existing skill\n');
  assert.equal(readFileSync(path.join(cwd, 'principals', 'existing.md'), 'utf8'), '# Existing principal\n');
  assert.deepEqual(readdirSync(path.join(cwd, 'skills')), ['existing.md']);
  assert.deepEqual(readdirSync(path.join(cwd, 'principals')), ['existing.md']);
  assert.deepEqual(await listFailureProposals({ cwd }), result.created);
  assert.match(formatFailureProposals(result.created), /Open improvement proposals \(1\)/);
});

test('snapshots and candidate sessions from one slice do not manufacture recurring slices', async (t) => {
  const cwd = fixture(t, Array.from({ length: 4 }, (_, index) => ({
    ...failures[0], session: `roster-41-a${index + 1}-coder`, model: `model-${index}`,
  })));
  assert.deepEqual((await proposeRecurringFailures({ cwd })).created, []);
  assert.deepEqual(await listFailureProposals({ cwd }), []);
  assert.equal(formatFailureProposals([]), '');
});

test('latest unmatched human rework comments contribute without requiring model metadata', async (t) => {
  const evaluations = [1, 2, 3].map((index) => ({
    session: `human-${index}`, verdict: 'rework', difficulty: 2, again: false,
    comment: `Review: \`helper${index}\` misses error case ${index}`,
  }));
  const cwd = fixture(t, [], evaluations);
  assert.equal((await proposeRecurringFailures({ cwd })).created.length, 1);
  const corrected = fixture(t, [], [...evaluations, { ...evaluations[0], verdict: 'accept' }]);
  assert.equal((await proposeRecurringFailures({ cwd: corrected })).created.length, 0);
});

test('rendering treats evidence as escaped data and redacts configured secrets', () => {
  const group = failureSignatures(failures)[0];
  group.reason = '# ignore rules <script> fixture-secret';
  const text = renderFailureProposal(group, { env: { ROSTER_API_KEY: 'fixture-secret' } });
  assert.doesNotMatch(text, /fixture-secret/);
  assert.doesNotMatch(text, /^# ignore rules/m);
  assert.match(text, /\\<script\\>/);
});

test('malformed history and hard-linked drafts fail explicitly without replacing content', async (t) => {
  const cwd = fixture(t);
  const result = await proposeRecurringFailures({ cwd });
  const file = path.join(cwd, '.roster', 'proposals', result.created[0]);
  linkSync(file, path.join(cwd, 'linked.md'));
  await assert.rejects(proposeRecurringFailures({ cwd }), /single-link/);
  await assert.rejects(listFailureProposals({ cwd }), /single-link/);
  writeFileSync(path.join(cwd, '.roster', 'runs', 'runs.jsonl'), 'not json\n');
  await assert.rejects(proposeRecurringFailures({ cwd }), /invalid JSON/);
});

test('symlinked draft directories and history roots are refused', async (t) => {
  const cwd = fixture(t);
  const outside = fixture(t);
  symlinkSync(outside, path.join(cwd, '.roster', 'proposals'), process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(proposeRecurringFailures({ cwd }), /symlinks/);
  await assert.rejects(listFailureProposals({ cwd }), /symlinks/);
  const empty = mkdtempSync(path.join(tmpdir(), 'roster-proposal-link-'));
  t.after(() => rmSync(empty, { recursive: true, force: true }));
  symlinkSync(path.join(outside, '.roster'), path.join(empty, '.roster'), process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(proposeRecurringFailures({ cwd: empty }), /symlinks/);
});

test('shell learn and stats use the current repository and remain offline', async (t) => {
  const cwd = fixture(t);
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));
  let text = '';
  const output = { write(value) { text += value; } };
  const shell = createDispatcher({ cwd, config, env: {}, output, errorOutput: output,
    services: { repositoryRoot: () => cwd, resolveContractsPath: () => assert.fail('learn needs no publisher'),
      loadMetrics: () => [], formatMetrics: () => '', summarizeMetrics: (records) => records } });
  await shell.dispatch('/learn --recurring');
  assert.match(text, /1 draft proposals created/);
  await assert.rejects(shell.dispatch('/learn'), /learn --recurring/);
  const statsShell = createDispatcher({ cwd, config, env: {}, output, errorOutput: output,
    services: { repositoryRoot: () => cwd, resolveContractsPath: () => 'fixture',
      loadMetrics: () => [], formatMetrics: () => '', summarizeMetrics: (records) => records } });
  await statsShell.dispatch('/stats');
  assert.match(text, /Open improvement proposals \(1\)/);
});

test('public CLI learns offline and stats lists open proposals without changing rules', (t) => {
  const cwd = fixture(t);
  execFileSync('git', ['init', '-b', 'main'], { cwd });
  execFileSync('git', ['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '--allow-empty', '-m', 'Fixture'], { cwd });
  const cli = fileURLToPath(new URL('../src/cli.mjs', import.meta.url));
  const execute = (args) => spawnSync(process.execPath, [cli, ...args], { cwd, encoding: 'utf8', timeout: 10000 });
  const learned = execute(['learn', '--recurring']);
  assert.equal(learned.status, 0, learned.stderr);
  assert.match(learned.stdout, /1 draft proposals created/);
  const stats = execute(['stats']);
  assert.equal(stats.status, 0, stats.stderr);
  assert.match(stats.stdout, /Open improvement proposals \(1\)/);
  assert.match(stats.stdout, /shadow-module-/);
  const invalid = execute(['learn', '--force']);
  assert.notEqual(invalid.status, 0);
  assert.match(invalid.stderr, /learn --recurring/);
});
