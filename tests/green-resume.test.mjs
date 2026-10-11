import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import test from 'node:test';
import { parseConfig } from '../src/lib/config.mjs';
import { readLocalRun } from '../src/lib/local-runs.mjs';
import { planStub, renderAssignment } from '../src/planner/stub.mjs';
import { captureCheckpoint, restoreVerifiedCheckpoint, listCheckpoints } from '../src/lib/checkpoints.mjs';
import { createDispatcher } from '../src/repl.mjs';

const sourceRoot = fileURLToPath(new URL('../', import.meta.url));
const config = parseConfig(readFileSync(path.join(sourceRoot, 'roster.config.example.yml'), 'utf8'));
function fixture(t) {
  const root = mkdtempSync(path.join(process.cwd(), '.green-resume-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', ['-c', 'user.name=fixture', '-c', 'user.email=fixture@example.invalid',
    '-c', 'commit.gpgsign=false', ...args], { cwd: root, stdio: 'pipe', encoding: 'utf8' });
  git('init', '-q', '-b', 'main');
  writeFileSync(path.join(root, 'app.mjs'), 'export const value = 1;\n');
  git('add', 'app.mjs'); git('commit', '-qm', 'fixture');
  git('remote', 'add', 'origin', 'https://github.com/example/project.git');
  const worktree = path.join(root, '.worktrees', 'issue-108');
  git('worktree', 'add', '-q', '-b', 'issue-108', worktree);
  const issue = { number: 108, title: 'Update app', body: 'Update app.mjs.', url: 'https://github.com/example/project/issues/108' };
  const plan = planStub(issue.body, { title: issue.title, reference: 'issue:108' });
  for (const [name, content] of Object.entries({ 'TASK.md': plan.task, 'RECIPE.yml': plan.recipe, 'ASSIGNMENT.md': renderAssignment(issue) })) writeFileSync(path.join(worktree, name), content);
  return { root, worktree, task: 'issue-108', allowedFiles: ['app.mjs'], env: {} };
}
const output = () => ({ text: '', write(text) { this.text += text; } });

test('fresh dispatcher green resume restores persistent task products and pauses; only Enter continues', async (t) => {
  const options = fixture(t);
  writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 2;\n');
  await captureCheckpoint({ ...options, verified: true, tests: { exit_code: 0 } });
  writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 999;\n');
  const writes = output(); let requests = 0; let preparedSeen;
  const shell = createDispatcher({ cwd: options.root, config, env: {}, output: writes, errorOutput: output(), services: {
    repositoryRoot: () => options.root,
    restoreVerifiedCheckpoint: (request) => restoreVerifiedCheckpoint({ ...request, hasPublishedPr: async () => false }),
    runBuiltinIssue: async (_issue, request) => {
      requests++; preparedSeen = request.preparedRun;
      assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 2;\n');
      return { ...request.preparedRun, confirmedPause: request.confirm === true, planningOnly: request.confirm === true, failed: false };
    },
  } });
  await shell.dispatch('/resume 108 --green');
  assert.equal(requests, 1);
  assert.equal(preparedSeen.worktreePath, options.worktree);
  assert.equal(shell.state.pendingConfirm.options.confirm, true);
  assert.match(writes.text, /Restored verified checkpoint/);
  assert.equal((await listCheckpoints(options)).length, 2);
  await shell.dispatch('/stop');
  await shell.dispatch('');
  assert.equal(requests, 1);
  await shell.dispatch('/resume 108 --green');
  await shell.dispatch('');
  assert.equal(requests, 3);
  assert.equal(shell.state.pendingConfirm, null);
});

test('green restore failure starts no run and PLAN-only requests do not restore', async (t) => {
  const options = fixture(t); let runs = 0; let restores = 0;
  const shell = createDispatcher({ cwd: options.root, config, env: {}, output: output(), errorOutput: output(), services: {
    restoreVerifiedCheckpoint: async () => { restores++; throw new Error('No verified green checkpoint'); },
    runBuiltinIssue: async () => { runs++; },
  } });
  await assert.rejects(shell.dispatch('/resume 108 --green'), /No verified green/);
  assert.equal(runs, 0);
  assert.equal(restores, 1);
  await assert.rejects(shell.dispatch('/resume 108 --green --publish'), /Use.*resume/);
});

test('CLI process restart restores actual product state, preserves later edits and pauses before coding', async (t) => {
  const options = fixture(t);
  writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 2;\n');
  await captureCheckpoint({ ...options, verified: true, tests: { exit_code: 0 } });
  writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 999;\n');
  const preload = path.join(options.root, 'offline-pr-check.mjs');
  writeFileSync(preload, "import child from 'node:child_process';\nimport { syncBuiltinESMExports } from 'node:module';\nimport { promisify } from 'node:util';\nconst original = child.execFile;\nchild.execFile = (program, args, options, callback) => {\n  if (program === 'gh') {\n    if (args[0] !== 'pr' || args[1] !== 'list') throw new Error('Unexpected GitHub operation');\n    queueMicrotask(() => callback(null, '[]', ''));\n    return { stdin: { end() {} } };\n  }\n  return original(program, args, options, callback);\n};\nchild.execFile[promisify.custom] = (program, args, options) => new Promise((resolve, reject) => child.execFile(program, args, options, (error, stdout, stderr) => error ? reject(error) : resolve({stdout, stderr})));\nsyncBuiltinESMExports();\n");
  const env = { PATH: process.env.PATH, SystemRoot: process.env.SystemRoot, USERPROFILE: process.env.USERPROFILE,
    GITHUB_AGENT_CONTRACTS: path.join(sourceRoot, 'vendor', 'github-agent-contracts'), ROSTER_PROVENANCE_OPT_OUT: '1' };
  const cli = path.join(sourceRoot, 'src', 'cli.mjs');
  const missingYes = spawnSync(process.execPath, [cli, 'resume', '108', '--green'], { cwd: options.root, env, encoding: 'utf8' });
  assert.equal(missingYes.status, 1);
  assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 999;\n');
  const result = spawnSync(process.execPath, ['--import', pathToFileURL(preload).href, cli, 'resume', '108', '--green', '--yes'],
    { cwd: options.root, env, encoding: 'utf8', timeout: 60000 });
  assert.equal(result.status, 0, result.stderr + result.stdout);
  assert.match(result.stdout, /Restored verified checkpoint/);
  assert.match(result.stdout, /Paused by --confirm/);
  assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 2;\n');
  assert.equal(existsSync(path.join(options.worktree, 'RESULT.md')), false);
  assert.equal(existsSync(path.join(options.worktree, 'REVIEW.md')), false);
  assert.equal((await listCheckpoints(options)).length, 2);
  writeFileSync(path.join(options.worktree, 'app.mjs'), 'export const value = 333;\n');
  const shellCode = 'import { createDispatcher } from ' + JSON.stringify(pathToFileURL(path.join(sourceRoot, 'src', 'repl.mjs')).href) + ';' +
    'const shell = createDispatcher({cwd: process.cwd(), services: {runBuiltinIssue: async (_issue, request) => ({...request.preparedRun, confirmedPause: true, planningOnly: true, failed: false})}}); await shell.dispatch("/resume 108 --green");';
  const shellProcess = spawnSync(process.execPath, ['--import', pathToFileURL(preload).href, '--input-type=module', '-e', shellCode],
    { cwd: options.root, env, encoding: 'utf8', timeout: 60000 });
  assert.equal(shellProcess.status, 0, shellProcess.stderr + shellProcess.stdout);
  assert.match(shellProcess.stdout, /Restored verified checkpoint/);
  assert.equal(readFileSync(path.join(options.worktree, 'app.mjs'), 'utf8'), 'export const value = 2;\n');
});
