import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import * as checkpoints from '../src/lib/checkpoints.mjs';
import { planStub, runtimeRecipe } from '../src/planner/stub.mjs';
import { withResearchSummary } from './helpers/research.mjs';
import { runCoder } from '../src/seats/coder.mjs';
import { execute } from '../src/runtime/exec.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { snapshotWorktree } from '../src/runtime/excellence.mjs';
import { acquireRepoLock } from '../src/lib/repo-locks.mjs';

function fixture(t) {
  const worktree = mkdtempSync(path.join(process.cwd(), '.verified-checkpoint-'));
  t.after(() => rmSync(worktree, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', ['-c', 'user.name=fixture', '-c', 'user.email=fixture@example.invalid',
    '-c', 'commit.gpgsign=false', ...args], { cwd: worktree, encoding: 'utf8', stdio: 'pipe' });
  git('init', '-q', '-b', 'issue-108');
  writeFileSync(path.join(worktree, 'app.mjs'), 'export const value = 1;\n');
  writeFileSync(path.join(worktree, 'TASK.md'), planStub('Update app.mjs.').task);
  writeFileSync(path.join(worktree, 'RECIPE.yml'), runtimeRecipe('issue:108'));
  git('add', 'app.mjs', 'TASK.md', 'RECIPE.yml'); git('commit', '-qm', 'baseline fixture');
  return { worktree, task: 'issue-108', allowedFiles: ['app.mjs'], env: {}, hasPublishedPr: async () => false, git };
}
const green = (options) => checkpoints.captureCheckpoint({ ...options, verified: true, tests: { exit_code: 0 } });

test('verified checkpoint survives restart and restores last green rather than latest unverified write', async (t) => {
  const options = fixture(t);
  writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 2;\n');
  const saved = await green(options);
  assert.equal(saved.status, 'verified');
  await checkpoints.captureCheckpoint(options);
  writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 999;\n');
  const before = readFileSync(path.join(options.worktree, 'TASK.md'), 'utf8');
  await checkpoints.restoreVerifiedCheckpoint(options);
  assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 2;\n');
  assert.equal(readFileSync(path.join(options.worktree, 'TASK.md'), 'utf8'), before);
  const records = await checkpoints.listCheckpoints(options);
  assert.equal(records.length, 3);
  await checkpoints.rewindCheckpoint({ ...options, number: records.at(-1).number });
  assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 999;\n');
});

for (const branch of ['agents/108', 'agents/108-a3', 'issue-108-a3']) {
  test('verified restore preserves stable task identity on compatible branch ' + branch, async (t) => {
    const options = fixture(t);
    options.git('checkout', '-qb', branch);
    writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 2;\n');
    await green(options);
    writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 999;\n');
    await checkpoints.restoreVerifiedCheckpoint(options);
    assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 2;\n');
    assert.equal(options.git('branch', '--show-current').trim(), branch);
  });
}

for (const branch of ['agents/109', 'agents/108-a4', 'agents/108-extra', 'human-work']) {
  test('verified restore refuses foreign or unsupported branch ' + branch, async (t) => {
    const options = fixture(t);
    options.git('checkout', '-qb', branch);
    await green(options);
    writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 999;\n');
    await assert.rejects(checkpoints.restoreVerifiedCheckpoint(options), /unchanged isolated task branch/);
    assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 999;\n');
  });
}

for (const tests of [{ exit_code: 1 }, { exit_code: 0, skipped: true }, undefined]) {
  test('failed/skipped/absent evidence cannot produce green checkpoint: ' + JSON.stringify(tests), async (t) => {
    const options = fixture(t);
    await assert.rejects(checkpoints.captureCheckpoint({ ...options, verified: true, tests }), /actual passing tests/);
    assert.deepEqual(await checkpoints.listCheckpoints(options), []);
  });
}

for (const kind of ['task', 'recipe', 'assignment', 'head', 'branch', 'scope', 'ref', 'published']) {
  test('changed ' + kind + ' refuses all green restoration', async (t) => {
    const options = fixture(t);
    const saved = await green(options);
    writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 999;\n');
    if (kind === 'task') writeFileSync(path.join(options.worktree, 'TASK.md'), planStub('Update app.mjs with a new constraint.').task);
    if (kind === 'recipe') writeFileSync(path.join(options.worktree, 'RECIPE.yml'), runtimeRecipe('issue:109'));
    if (kind === 'assignment') writeFileSync(path.join(options.worktree, 'ASSIGNMENT.md'), 'Changed issue authority');
    if (kind === 'head') options.git('commit', '--allow-empty', '-qm', 'changed baseline');
    if (kind === 'branch') options.git('checkout', '-qb', 'another-task');
    if (kind === 'scope') options.allowedFiles = ['app.mjs', 'extra.mjs'];
    if (kind === 'ref') options.git('update-ref', saved.ref, options.git('rev-parse', 'HEAD^{tree}').trim());
    if (kind === 'published') options.hasPublishedPr = async () => true;
    await assert.rejects(checkpoints.restoreVerifiedCheckpoint(options), /changed|published|match task identity/);
    assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 999;\n');
  });
}

test('active writer lock and cancellation prevent restoration', async (t) => {
  const options = fixture(t); await green(options);
  const lock = await acquireRepoLock('issue-108', { lockRoot: path.join(options.worktree, '.git', 'roster', 'wave-runs') });
  await assert.rejects(checkpoints.restoreVerifiedCheckpoint(options), /held/);
  await lock.release();
  const controller = new AbortController(); controller.abort(new Error('cancelled'));
  await assert.rejects(checkpoints.restoreVerifiedCheckpoint({ ...options, signal: controller.signal }), /cancel/i);
});

test('changed post-test product snapshot cannot be labeled green', async (t) => {
  const options = fixture(t);
  const expectedSnapshot = await snapshotWorktree(options.worktree);
  writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 9;\n');
  await assert.rejects(checkpoints.captureCheckpoint({ ...options, verified: true, tests: { exit_code: 0 }, expectedSnapshot }), /changed after verified/);
  assert.deepEqual(await checkpoints.listCheckpoints(options), []);
});

for (const kind of ['invalid', 'hardlink', 'junction']) {
  test('unsafe checkpoint storage blocks restoration: ' + kind, async (t) => {
    const options = fixture(t); await green(options);
    const directory = path.join(options.worktree, '.roster', 'checkpoints', '108');
    const file = path.join(directory, '1');
    if (kind === 'invalid') writeFileSync(file, '{bad');
    if (kind === 'hardlink') linkSync(file, path.join(options.worktree, 'metadata-copy'));
    if (kind === 'junction') {
      const target = path.join(options.worktree, 'unsafe-state');
      mkdirSync(target); rmSync(directory, { recursive: true });
      symlinkSync(target, directory, process.platform === 'win32' ? 'junction' : 'dir');
    }
    await assert.rejects(checkpoints.restoreVerifiedCheckpoint(options), /metadata|single-link|symlink/);
    assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 1;\n');
  });
}

test('actual passing coder verification persists green product snapshot; later repair can restore it', async (t) => {
  const options = fixture(t);
  writeFileSync(path.join(options.worktree, 'app.test.mjs'), "import test from 'node:test';\nimport assert from 'node:assert/strict';\nimport { value } from './app.mjs';\ntest('updated value', () => assert.equal(value, 2));\n");
  options.allowedFiles.push('app.test.mjs');
  writeFileSync(path.join(options.worktree, 'TASK.md'), planStub('Update app.mjs and app.test.mjs.').task);
  options.git('add', 'app.test.mjs', 'TASK.md'); options.git('commit', '-qm', 'pin failing value fixture');
  const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8')
    .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: fixture-model'));
  let requests = 0;
  const result = await runCoder({ ...options, repoRoot: fileURLToPath(new URL('../', import.meta.url)), session: 'checkpoint-fixture', config,
    askKind: 'slice', env: { PATH: process.env.PATH, SystemRoot: process.env.SystemRoot },
    runTestCommand: (program, args, request) => execute(program === 'node' ? process.execPath : program, args, request),
    fetchImpl: withResearchSummary(async (url, request) => {
      requests++;
      const calls = requests === 1 ? [
        ['read_file', { path: 'app.mjs' }],
        ['edit_file', { path: 'app.mjs', old_string: 'value = 1', new_string: 'value = 2' }],
      ] : null;
      const message = calls ? { role: 'assistant', tool_calls: calls.map(([name,args],i) => ({ id: 'tool'+i,
        type: 'function', function: { name, arguments: JSON.stringify(args) } })) }
        : { role: 'assistant', content: 'Updated value with actual passing tests.' };
      return Response.json({ choices: [{ finish_reason: calls ? 'tool_calls' : 'stop', message }] });
    }),
  });
  assert.equal(result.excellence.pass, true);
  assert.equal(result.tests.exit_code, 0);
  const records = await checkpoints.listCheckpoints(options);
  assert.ok(records.some(({ status }) => status === 'verified'));
  writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 999;\n');
  await checkpoints.restoreVerifiedCheckpoint(options);
  assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 2;\n');
});

test('restoration retires stale passing evidence without discarding its archive', async (t) => {
  const options = fixture(t); await green(options);
  for (const file of ['RESULT.md', 'REVIEW.md']) writeFileSync(path.join(options.worktree, file), 'Previous passing evidence');
  await checkpoints.restoreVerifiedCheckpoint(options);
  for (const file of ['RESULT.md', 'REVIEW.md']) assert.equal(existsSync(path.join(options.worktree, file)), false);
  const archive = path.join(options.worktree, '.git', 'roster-artifacts', options.task);
  const directory = (await import('node:fs/promises')).readdir(archive);
  assert.equal(readFileSync(path.join(archive, (await directory)[0], 'REVIEW.md'), 'utf8'), 'Previous passing evidence');
});

test('unknown green and secret later edits refuse without replacing products', async (t) => {
  const options = fixture(t);
  await assert.rejects(checkpoints.restoreVerifiedCheckpoint(options), /No verified green/);
  await green(options);
  options.env = { CUSTOM_SERVICE_KEY: 'checkpoint-secret-private-value-123456' };
  writeFileSync(path.join(options.worktree, 'app.mjs'), options.env.CUSTOM_SERVICE_KEY);
  await assert.rejects(checkpoints.restoreVerifiedCheckpoint({ ...options, apiKeyEnv: 'CUSTOM_SERVICE_KEY' }), /secret/);
  assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), options.env.CUSTOM_SERVICE_KEY);
  assert.equal((await checkpoints.listCheckpoints(options)).length, 1);
});
