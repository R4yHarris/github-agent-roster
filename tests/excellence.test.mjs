import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { prepareBuiltinPublication } from '../src/lib/builtin.mjs';
import { planStub } from '../src/planner/stub.mjs';
import { checkExcellence, declaredTestNames, snapshotWorktree, taskSkipsTests, testEvidence, writeResult } from '../src/runtime/excellence.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { withResearchSummary } from './helpers/research.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
  .replace('base_url: ""', 'base_url: http://localhost:3456/v1').replace('model: ""', 'model: local-model'));

async function fixture(context) {
  const repoRoot = mkdtempSync(path.join(tmpdir(), 'roster-excellence-'));
  context.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  const worktree = path.join(repoRoot, 'worktree');
  mkdirSync(worktree);
  const task = planStub('Update README.md.').task;
  writeFileSync(path.join(worktree, 'README.md'), '# Before\n');
  writeFileSync(path.join(worktree, 'TASK.md'), task);
  const baseline = await snapshotWorktree(worktree);
  return { repoRoot, worktree, task, baseline, env: {}, result: {
    mode: 'llm', model: 'local-model', turns: 2, tests: { exit_code: 0 },
    summary: 'README updated with test evidence.',
  } };
}

test('clean fixture passes with changed scope, executed tests, and recorded model/turns', async (context) => {
  const options = await fixture(context);
  writeFileSync(path.join(options.worktree, 'README.md'), '# After\n');
  const gate = await checkExcellence(options);
  const { snapshot, ...checks } = gate;
  assert.ok(snapshot instanceof Map);
  assert.deepEqual(checks, { pass: true, reasons: [], files: ['README.md'], model: 'local-model', turns: 2 });
  const file = await writeResult({ ...options, excellence: gate });
  assert.match(readFileSync(file, 'utf8'), /Checks: PASS/);
  assert.match(readFileSync(file, 'utf8'), /node --test exited 0/);
  assert.match(readFileSync(file, 'utf8'), /Model: local-model\nTool-loop turns: 2/);
});

test('a changed vendor submodule path is excluded from the reviewed application diff', async (context) => {
  const options = await fixture(context);
  const git = (cwd, ...args) => execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.invalid',
    '-c', 'commit.gpgsign=false', ...args], { cwd, stdio: 'pipe' });
  const vendor = path.join(options.worktree, 'vendor', 'github-agent-contracts');
  mkdirSync(vendor, { recursive: true });
  git(vendor, 'init', '-q');
  writeFileSync(path.join(vendor, 'agent-pr.mjs'), 'first\n');
  git(vendor, 'add', '.');
  git(vendor, 'commit', '-q', '-m', 'first');
  git(options.worktree, 'init', '-q');
  git(options.worktree, 'add', 'README.md', 'TASK.md', 'vendor/github-agent-contracts');
  git(options.worktree, 'commit', '-q', '-m', 'base');
  options.baseline = await snapshotWorktree(options.worktree);
  writeFileSync(path.join(vendor, 'agent-pr.mjs'), 'second\n');
  git(vendor, 'commit', '-q', '-am', 'second');
  writeFileSync(path.join(options.worktree, 'README.md'), '# After\n');
  const gate = await checkExcellence(options);
  assert.equal(options.result.tests.exit_code, 0);
  assert.equal(gate.pass, true);
  assert.deepEqual(gate.reasons, []);
  assert.deepEqual(gate.files, ['README.md']);
});

test('secret paths and out-of-scope changes fail without reading or reporting secret values', async (context) => {
  const options = await fixture(context);
  writeFileSync(path.join(options.worktree, '.env'), 'PRIVATE=fixture-secret-value\n');
  writeFileSync(path.join(options.worktree, 'outside.txt'), 'unrelated');
  const gate = await checkExcellence(options);
  assert.equal(gate.pass, false);
  assert.ok(gate.reasons.some((reason) => reason.includes('.env')));
  assert.ok(gate.reasons.some((reason) => reason.includes('outside.txt')));
  const file = await writeResult({ ...options, excellence: gate });
  const report = readFileSync(file, 'utf8');
  assert.match(report, /Checks: FAIL[\s\S]*First failure:/);
  assert.doesNotMatch(report, /fixture-secret-value/);
});

test('known secret material in an allowed file fails without echoing the credential', async (context) => {
  const options = await fixture(context);
  options.env = { CUSTOM_KEY: 'fixture-only-private-value' };
  options.apiKeyEnv = 'CUSTOM_KEY';
  writeFileSync(path.join(options.worktree, 'README.md'), 'fixture-only-private-value');
  const gate = await checkExcellence(options);
  assert.equal(gate.pass, false);
  assert.match(gate.reasons.join('\n'), /Secret material.*README\.md/);
  assert.doesNotMatch(gate.reasons.join('\n'), /fixture-only-private-value/);
});

test('PEM envelope prose and keyless fixtures pass; base64 key bodies fail with line numbers', async (context) => {
  const options = await fixture(context);
  writeFileSync(path.join(options.worktree, 'README.md'), [
    '# Redaction',
    'PEM blocks: the `-----BEGIN ... PRIVATE KEY-----` / `-----END ... PRIVATE KEY-----` envelope.',
    "const pem = '-----BEGIN PRIVATE KEY-----\\n' + body + '\\n-----END PRIVATE KEY-----';",
  ].join('\n'));
  assert.equal((await checkExcellence(options)).reasons.some((reason) => reason.startsWith('Secret material')), false);
  writeFileSync(path.join(options.worktree, 'README.md'),
    `# Key\n\n-----BEGIN ${'RSA'} PRIVATE KEY-----\n${'MIIE'}${'A'.repeat(60)}\n-----END RSA PRIVATE KEY-----\n`);
  const gate = await checkExcellence(options);
  assert.match(gate.reasons.join('\n'), /Secret material detected in changed file: README\.md \(line 3\)/);
});

test('edits inside existing protected directories fail in non-Git fixtures', async (context) => {
  const options = await fixture(context);
  mkdirSync(path.join(options.worktree, '.github', 'workflows'), { recursive: true });
  const workflow = path.join(options.worktree, '.github', 'workflows', 'ci.yml');
  writeFileSync(workflow, 'original');
  options.baseline = await snapshotWorktree(options.worktree);
  writeFileSync(workflow, 'modified workflow');
  const gate = await checkExcellence(options);
  assert.equal(gate.pass, false);
  assert.ok(gate.reasons.some((reason) => reason.includes('.github/workflows/ci.yml')));
});

test('only initial task frontmatter can waive tests and metadata must still be recorded', async (context) => {
  const options = await fixture(context);
  options.result = { ...options.result, tests: undefined };
  assert.equal((await checkExcellence(options)).pass, false);
  const task = options.task.replace('---\n', '---\ntests: none\n');
  assert.equal(taskSkipsTests(task), true);
  assert.equal(taskSkipsTests(options.task + '\ntests: none\n'), false);
  assert.equal((await checkExcellence({ ...options, task })).pass, true);
  assert.equal((await checkExcellence({
    ...options, task, result: { ...options.result, model: '', turns: undefined },
  })).pass, false);
});

test('edits after final verification invalidate an earlier passing gate', async (context) => {
  const options = await fixture(context);
  const verified = await checkExcellence(options);
  assert.equal(verified.pass, true);
  writeFileSync(path.join(options.worktree, 'README.md'), '# Later change\n');
  const gate = await checkExcellence({ ...options, verifiedSnapshot: verified.snapshot });
  assert.equal(gate.pass, false);
  assert.match(gate.reasons.join('\n'), /changed after final verification/);
});

test('a bounded regression task cannot pass on existing green tests without an actual Git diff', async (t) => {
  const options = await fixture(t);
  const task = planStub('Add a regression in tests/smoke.test.mjs.').task;
  mkdirSync(path.join(options.worktree, 'tests'));
  const file = path.join(options.worktree, 'tests', 'smoke.test.mjs');
  const content = "import test from 'node:test';\ntest('existing', () => {});\n";
  writeFileSync(file, content);
  const git = (...args) => execFileSync('git', args, { cwd: options.worktree, stdio: 'pipe' });
  git('init', '--quiet');
  git('add', '--all');
  git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--quiet', '-m', 'baseline');
  const baseline = await snapshotWorktree(options.worktree);
  writeFileSync(file, content);
  const gate = await checkExcellence({ ...options, task, baseline });
  assert.equal(gate.pass, false);
  assert.deepEqual(gate.files, []);
  assert.match(gate.reasons.join('\n'), /must produce an application diff[\s\S]*passing existing tests/);
  writeFileSync(file, content + "test('regression', () => {});\n");
  assert.equal((await checkExcellence({ ...options, task, baseline })).pass, true);
  writeFileSync(file, content);
  assert.equal((await checkExcellence({ ...options, task, baseline })).pass, false,
    'Reverting a prior write must not retain implementation success');
});

test('test substance flags only added tests that cannot fail, never pre-existing ones', async (context) => {
  const options = await fixture(context);
  options.task = planStub('Add a regression test in tests/route.test.mjs.').task;
  writeFileSync(path.join(options.worktree, 'TASK.md'), options.task);
  mkdirSync(path.join(options.worktree, 'src'));
  mkdirSync(path.join(options.worktree, 'tests'));
  writeFileSync(path.join(options.worktree, 'src', 'route.mjs'), 'export const formatRoute = (env) => `route ${Object.keys(env).length}`;\n');
  const file = path.join(options.worktree, 'tests', 'route.test.mjs');
  const original = "import assert from 'node:assert/strict';\nimport test from 'node:test';\n" +
    "import { formatRoute } from '../src/route.mjs';\n\ntest('legacy shape', () => {\n  assert.equal(1, 1);\n});\n";
  writeFileSync(file, original);
  const git = (...args) => execFileSync('git', args, { cwd: options.worktree, encoding: 'utf8', stdio: 'pipe' });
  git('init', '--quiet');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'Fixture');
  options.baseline = await snapshotWorktree(options.worktree);
  writeFileSync(file, `${original}\ntest('summary hides keys', () => {\n  const summary = { key: 'test-only-private-api-key' };\n` +
    "  delete summary.key;\n  assert.ok(!JSON.stringify(summary).includes('test-only-private-api-key'));\n});\n");
  const weak = await checkExcellence(options);
  assert.equal(weak.pass, false);
  assert.equal(weak.reasons.length, 2);
  assert.match(weak.reasons[0], /^Test substance: new test "summary hides keys" in tests\/route\.test\.mjs never calls/);
  assert.match(weak.reasons[1], /^Test substance: sentinel 'test-only-private-api-key' in tests\/route\.test\.mjs is asserted absent/);
  writeFileSync(file, `${original}\ntest('summary hides keys', () => {\n  const secret = 'test-only-private-api-key';\n` +
    '  assert.ok(!formatRoute({ ROSTER_API_KEY: secret }).includes(secret));\n});\n');
  assert.deepEqual((await checkExcellence(options)).reasons, []);
  writeFileSync(file, original.replace("import { formatRoute } from '../src/route.mjs';",
    "import { formatRoute } from '../src/route.mjs';\nimport { strict } from 'node:assert';\n// planned regression test"));
  assert.deepEqual((await checkExcellence(options)).reasons, ['Test substance: the diff changes only test files but adds no new test block or assertion; ' +
    'imports, comments, or fixtures alone do not implement the Ask.']);
  writeFileSync(file, original.replace("assert.equal(1, 1);", "assert.equal(formatRoute({}), 'route 0');"));
  assert.deepEqual((await checkExcellence(options)).reasons, [], 'Strengthening an existing assertion is real test work');
});

test('Git diff checks include out-of-scope changes that already existed before the loop', async (context) => {
  const options = await fixture(context);
  const git = (...args) => execFileSync('git', args, { cwd: options.worktree, encoding: 'utf8', stdio: 'pipe' });
  git('init', '--quiet');
  git('add', '--all');
  git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '--quiet', '-m', 'Fixture');
  writeFileSync(path.join(options.worktree, 'outside.txt'), 'preexisting untracked change');
  options.baseline = await snapshotWorktree(options.worktree);
  const gate = await checkExcellence(options);
  assert.equal(gate.pass, false);
  assert.match(gate.reasons.join('\n'), /outside\.txt/);
});

test('an explicit no-tests task skips automatic tests but still writes a gated result', async (context) => {
  const options = await fixture(context);
  cpSync(new URL('../principals/', import.meta.url), path.join(options.repoRoot, 'principals'), { recursive: true });
  cpSync(new URL('../skills/', import.meta.url), path.join(options.repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(options.worktree, 'AGENTS.md'), '# Instructions\nStay scoped.\n');
  writeFileSync(path.join(options.worktree, 'TASK.md'), options.task.replace('---\n', '---\ntests: none\n'));
  let turns = 0;
  const result = await runCoder({
    ...options, config, task: 'issue-4', session: 'coder-4', vault: { get: async () => undefined },
    fetchImpl: withResearchSummary(async () => {
      turns += 1;
      return Response.json({ choices: [turns === 1 ? {
        finish_reason: 'tool_calls',
        message: { role: 'assistant', tool_calls: [{ id: 'write', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({
            path: 'README.md', content: '# Bounded no-tests result\n',
          }),
        } }] },
      } : {
        finish_reason: 'stop', message: { role: 'assistant', content: 'No tests requested.' },
      }] });
    }),
    runTestCommand: () => assert.fail('Explicit no-tests task must not automatically execute tests'),
  });
  assert.equal(result.excellence.pass, true);
  assert.equal(result.testsSkipped, true);
  assert.equal(result.tests, undefined);
  const report = readFileSync(result.resultPath, 'utf8');
  assert.match(report, /Tests skipped: docs-only change is checked by reading the file\./);
  assert.doesNotMatch(report, /node --test exited/);
});

test('a test subprocess scope violation writes a failed result and cannot reach publication', async (context) => {
  const options = await fixture(context);
  cpSync(new URL('../principals/', import.meta.url), path.join(options.repoRoot, 'principals'), { recursive: true });
  cpSync(new URL('../skills/', import.meta.url), path.join(options.repoRoot, 'skills'), { recursive: true });
  writeFileSync(path.join(options.worktree, 'AGENTS.md'), '# Instructions\nStay scoped.\n');
  const task = planStub('Update src/runtime/excellence.mjs.').task;
  mkdirSync(path.join(options.worktree, 'src', 'runtime'), { recursive: true });
  mkdirSync(path.join(options.worktree, 'tests'), { recursive: true });
  writeFileSync(path.join(options.worktree, 'src', 'runtime', 'excellence.mjs'), 'export const value = 1;\n');
  writeFileSync(path.join(options.worktree, 'tests', 'excellence.test.mjs'), 'export {};\n');
  writeFileSync(path.join(options.worktree, 'TASK.md'), task);
  let failure;
  let turns = 0;
  await assert.rejects(runCoder({
    ...options, config, task: 'issue-4', session: 'coder-4', vault: { get: async () => undefined },
    fetchImpl: withResearchSummary(async () => {
      turns += 1;
      return Response.json({ choices: [turns === 1 ? {
        finish_reason: 'tool_calls',
        message: { role: 'assistant', tool_calls: [{ id: 'write', type: 'function', function: {
          name: 'write_file', arguments: JSON.stringify({
            path: 'src/runtime/excellence.mjs', content: 'export const value = 2;\n',
          }),
        } }] },
      } : {
        finish_reason: 'stop', message: { role: 'assistant', content: 'Updated excellence behavior.' },
      }] });
    }),
    runTestCommand: async () => {
      writeFileSync(path.join(options.worktree, 'outside.txt'), 'test side effect');
      return { exit_code: 0, stdout: 'tests pass', stderr: '' };
    },
  }), (error) => { failure = error; return /outside TASK\.md/.test(error.message); });
  const report = readFileSync(path.join(options.worktree, 'RESULT.md'), 'utf8');
  assert.match(report, /Checks: FAIL[\s\S]*outside\.txt/);
  const memory = readFileSync(path.join(options.repoRoot, '.roster', 'memory', 'coder.jsonl'), 'utf8')
    .trimEnd().split('\n').map(JSON.parse);
  assert.equal(memory.at(-1).status, 'failed');
  await assert.rejects(prepareBuiltinPublication({
    worktreePath: options.worktree, planner: { task, recipe: 'recipe' },
    runs: { coder: { env: {} } }, result: failure.result,
  }, { config, env: {} }), /passing excellence gate/);
});

test('a regression repair from an earlier coder attempt stays in scope for the next perspective', async (context) => {
  const setup = async () => {
    const options = await fixture(context);
    cpSync(new URL('../principals/', import.meta.url), path.join(options.repoRoot, 'principals'), { recursive: true });
    cpSync(new URL('../skills/', import.meta.url), path.join(options.repoRoot, 'skills'), { recursive: true });
    writeFileSync(path.join(options.worktree, 'AGENTS.md'), '# Instructions\nStay scoped.\n');
    const task = planStub('Update src/runtime/excellence.mjs.').task;
    mkdirSync(path.join(options.worktree, 'src', 'runtime'), { recursive: true });
    mkdirSync(path.join(options.worktree, 'tests'), { recursive: true });
    writeFileSync(path.join(options.worktree, 'src', 'runtime', 'excellence.mjs'), 'export const value = 1;\n');
    const other = path.join(options.worktree, 'tests', 'other.test.mjs');
    const original = "import assert from 'node:assert/strict';\nimport test from 'node:test';\n" +
      "import { value } from '../src/runtime/excellence.mjs';\n\ntest('value', () => {\n  assert.equal(value, 1);\n});\n";
    writeFileSync(other, original);
    writeFileSync(path.join(options.worktree, 'TASK.md'), task);
    const git = (...args) => execFileSync('git', args, { cwd: options.worktree, encoding: 'utf8', stdio: 'pipe' });
    git('init', '--quiet');
    git('add', '--all');
    git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'Fixture');
    // The earlier attempt repaired a test outside Allowed Files that its change broke.
    writeFileSync(other, original.replace('assert.equal(value, 1)', 'assert.equal(value, 2)'));
    return options;
  };
  const run = (options, initialRepairFiles) => {
    let turns = 0;
    return runCoder({
      ...options, config, task: 'issue-4', session: 'coder-4', vault: { get: async () => undefined },
      initialRepairFiles,
      fetchImpl: withResearchSummary(async () => {
        turns += 1;
        return Response.json({ choices: [turns === 1 ? {
          finish_reason: 'tool_calls',
          message: { role: 'assistant', tool_calls: [{ id: 'write', type: 'function', function: {
            name: 'write_file', arguments: JSON.stringify({
              path: 'src/runtime/excellence.mjs', content: 'export const value = 2;\n',
            }),
          } }] },
        } : {
          finish_reason: 'stop', message: { role: 'assistant', content: 'Updated value; kept the earlier test repair.' },
        }] });
      }),
      runTestCommand: async () => ({ exit_code: 0, stdout: 'tests pass', stderr: '' }),
    });
  };
  await assert.rejects(run(await setup(), []), /outside TASK\.md allowed paths: tests\/other\.test\.mjs/);
  const result = await run(await setup(), ['tests/other.test.mjs']);
  assert.equal(result.excellence.pass, true);
  assert.deepEqual(result.repairFiles, ['tests/other.test.mjs']);
});

test('test evidence quotes suite totals and every result from changed test files, not just the output head', () => {
  const names = declaredTestNames("test('dry-run touches nothing', () => {});\ntest(\"it\\'s idempotent\", () => {});\n" +
    'for (const x of [1]) test(`loop ${x}`, () => {});\n');
  assert.deepEqual(names, ['dry-run touches nothing', "it's idempotent"]);
  const head = Array.from({ length: 200 }, (_, index) => `✔ unrelated test ${index} (1.0ms)`).join('\n');
  const stdout = `${head}\n✔ dry-run touches nothing (3.1ms)\n✔ it's idempotent (2.0ms)\n` +
    'ℹ tests 202\nℹ suites 0\nℹ pass 202\nℹ fail 0\nℹ skipped 0\n';
  const evidence = testEvidence({ exit_code: 0, stdout, stderr: '' },
    [{ file: 'tests/repo-migrate.test.mjs', names }]);
  assert.match(evidence, /Totals:\n  ℹ tests 202\n  ℹ suites 0\n  ℹ pass 202\n  ℹ fail 0/);
  assert.match(evidence, /tests\/repo-migrate\.test\.mjs: 2 of 2 declared tests reported\n  ✔ dry-run touches nothing \(3\.1ms\)\n  ✔ it's idempotent/);
  assert.match(evidence, /Output:\n✔ unrelated test 0/);
  assert.doesNotMatch(testEvidence({ exit_code: 0, stdout: 'ok', stderr: '' }), /Changed test files|Totals/);
});
