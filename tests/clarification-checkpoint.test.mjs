import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { chmodSync, existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { PassThrough } from 'node:stream';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { createDispatcher, startRepl } from '../src/repl.mjs';
import { parseConfig } from '../src/lib/config.mjs';
import { readClarification, saveClarification, cancelClarification, resumeClarification } from '../src/lib/clarification-checkpoint.mjs';

const config = parseConfig(readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8'));
const cli = fileURLToPath(new URL('../src/cli.mjs', import.meta.url));
const vague = 'Improve the product; keep the public API unchanged.';
const answer = 'Add a Status heading.\n\n## Allowed Files\n- `README.md`\n\n## Acceptance checks\n- README has a Status heading';
const file = (root) => join(root, '.roster', 'asks', 'clarification.json');

function fixture(t, { ignored = true } = {}) {
  const root = mkdtempSync(join(process.cwd(), '.intake-checkpoint-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  execFileSync('git', ['init', '--quiet'], { cwd: root });
  if (ignored) writeFileSync(join(root, '.gitignore'), '.roster/asks/\n');
  return root;
}

function shell(root, services = {}) {
  let text = '';
  const output = { write: (value) => { text += value; } };
  const result = createDispatcher({ cwd: root, config, env: {}, output, errorOutput: output,
    services: { repositoryBranch: () => 'main', runBuiltinAsk: async (_ask, { log }) => {
      log('Ask kind: clarify');
      return { askKind: 'clarify', planningOnly: true, failed: false };
    }, ...services } });
  return { ...result, get text() { return text; } };
}

function run(root, args) {
  const gitPath = execFileSync('git', ['--exec-path'], { encoding: 'utf8' }).trim();
  assert.ok(existsSync(join(gitPath, process.platform === 'win32' ? 'git.exe' : 'git')));
  return spawnSync(process.execPath, [cli, ...args], { cwd: root, encoding: 'utf8',
    env: { ...process.env, PATH: gitPath, ROSTER_API_KEY: '', OPENAI_API_KEY: '', GITHUB_APP_ID: '',
      GITHUB_APP_PRIVATE_KEY_PATH: '', ROSTER_PROVENANCE_OPT_OUT: 'true' }, timeout: 15000 });
}

test('CLI vague intake survives process restart; validated resume writes an offline draft in the project', (t) => {
  const root = fixture(t);
  const first = run(root, ['ask', vague]);
  assert.equal(first.status, 0, first.stderr);
  assert.match(first.stdout, /Checkpoint saved privately/);
  assert.ok(existsSync(file(root)));
  const pending = run(root, ['ask', '--pending']);
  assert.equal(pending.status, 0, pending.stderr);
  assert.match(pending.stdout, /keep the public API unchanged/);
  assert.match(pending.stdout, /Assumptions[\s\S]*Questions/);
  const invalid = run(root, ['ask', '--resume', 'Add a Status section to README.md.']);
  assert.notEqual(invalid.status, 0);
  assert.match(invalid.stderr, /explicitly declare Allowed Files/);
  assert.ok(existsSync(file(root)));
  const resumed = run(root, ['ask', '--resume', answer]);
  assert.equal(resumed.status, 0, resumed.stderr);
  assert.match(resumed.stdout, /Ask kind: slice[\s\S]*RECIPE:[\s\S]*TASK:/);
  assert.equal(existsSync(file(root)), false);
  const askPath = /^Ask: (.+)$/m.exec(resumed.stdout)[1];
  assert.ok(askPath.startsWith(root));
  assert.match(readFileSync(askPath, 'utf8'), /keep the public API unchanged[\s\S]*## Clarification[\s\S]*README.md/);
  assert.match(run(root, ['ask', '--pending']).stdout, /No pending clarification/);
});

test('CLI cancellation after restart removes only the checkpoint; malformed data never resumes', (t) => {
  const root = fixture(t);
  assert.equal(run(root, ['ask', vague]).status, 0);
  writeFileSync(file(root), '{broken');
  const invalid = run(root, ['ask', '--resume', answer]);
  assert.notEqual(invalid.status, 0);
  assert.match(invalid.stderr, /Invalid clarification checkpoint/);
  assert.equal(run(root, ['ask', '--cancel']).status, 0);
  assert.equal(existsSync(file(root)), false);
  assert.match(run(root, ['ask', '--cancel']).stdout, /No pending clarification/);
});

test('REPL reloads pending intake, refuses implicit/protected scope, then resumes through the ordinary seat boundary', async (t) => {
  const root = fixture(t);
  const first = shell(root);
  await first.dispatch(vague);
  assert.match(first.text, /Checkpoint saved privately/);
  const observed = [];
  const next = shell(root, { runBuiltinAsk: async (ask, options) => {
    observed.push({ ask, options });
    return { askKind: 'slice', failed: false, planningOnly: true, confirmedPause: true };
  } });
  await next.dispatch('/clarify');
  assert.match(next.text, /Original Ask:[\s\S]*keep the public API unchanged/);
  await assert.rejects(next.dispatch('/clarify Add Status to README.md.'), /explicitly declare/);
  await assert.rejects(next.dispatch('/clarify Add Status.\n\n## Allowed Files\n- `../escape.md`'), /Files allowed/);
  await assert.rejects(next.dispatch('/clarify Add Status.\n\n## Allowed Files\n- `.github/workflows/ci.yml`'), /Files allowed/);
  assert.equal(observed.length, 0);
  await next.dispatch(`/clarify ${answer}`);
  assert.equal(observed.length, 1);
  assert.match(observed[0].ask, /keep the public API unchanged[\s\S]*## Clarification/);
  assert.equal(observed[0].options.publish, false);
  assert.equal(observed[0].options.confirm, true);
  assert.match(next.text, /remains pending/);
  assert.equal((await readClarification({ repoRoot: root, env: {} })).answer, answer);
  const restarted = shell(root, { runBuiltinAsk: async (ask, options) => {
    assert.match(ask, /keep the public API unchanged/);
    assert.equal(options.confirm, true);
    return { askKind: 'slice', failed: false };
  } });
  await restarted.dispatch('/clarify continue');
  assert.match(restarted.text, /Clarification resumed/);
  assert.equal(existsSync(file(root)), false);
});

test('cancelled or failed shell execution retains intake across another restart; explicit cancellation runs no seats', async (t) => {
  const root = fixture(t);
  await saveClarification(vague, { repoRoot: root, env: {} });
  const aborted = shell(root, { runBuiltinAsk: async () => { throw new Error('cancelled test run'); } });
  await assert.rejects(aborted.dispatch(`/clarify ${answer}`), /cancelled test run/);
  assert.ok(await readClarification({ repoRoot: root, env: {} }));
  const failed = shell(root, { runBuiltinAsk: async () => ({ askKind: 'slice', failed: true }) });
  await failed.dispatch(`/clarify ${answer}`);
  assert.match(failed.text, /remains pending/);
  const restarted = shell(root, { runBuiltinAsk: () => assert.fail('Cancellation must not run a seat') });
  await restarted.dispatch('/clarify cancel');
  assert.match(restarted.text, /cancelled. No new execution or publication/);
  assert.equal(existsSync(file(root)), false);
});

test('checkpoint preserves original constraints, redacts secrets and refuses secret-bearing resume', async (t) => {
  const root = fixture(t);
  const env = { UNUSUAL_CREDENTIAL: 'checkpoint-secret-value' };
  const options = { repoRoot: root, env, apiKeyEnv: 'UNUSUAL_CREDENTIAL' };
  const record = await saveClarification('Improve things with checkpoint-secret-value', options);
  assert.doesNotMatch(readFileSync(file(root), 'utf8'), /checkpoint-secret-value/);
  assert.equal((await readClarification(options)).id, record.id);
  await assert.rejects(resumeClarification(answer + '\ncheckpoint-secret-value', options), /Remove secrets/);
  await assert.rejects(saveClarification('Fix the bug.', options), /already pending/);
  await assert.rejects(cancelClarification({ ...options, id: 'not-the-id' }), /changed/);
  assert.ok(existsSync(file(root)));
});

test('private storage rejects unignored or tracked files, symlink components and nonregular or oversized checkpoints', async (t) => {
  const unignored = fixture(t, { ignored: false });
  await assert.rejects(saveClarification(vague, { repoRoot: unignored, env: process.env }), /untracked and gitignored/);
  assert.equal(existsSync(file(unignored)), false);
  const tracked = fixture(t);
  await saveClarification(vague, { repoRoot: tracked });
  execFileSync('git', ['add', '--force', '.roster/asks/clarification.json'], { cwd: tracked });
  await assert.rejects(readClarification({ repoRoot: tracked }), /untracked and gitignored/);
  const root = fixture(t);
  mkdirSync(file(root), { recursive: true });
  await assert.rejects(readClarification({ repoRoot: root }), /private, bounded regular/);
  rmSync(file(root), { recursive: true });
  writeFileSync(file(root), ' '.repeat(65537), { mode: 0o600 });
  await assert.rejects(readClarification({ repoRoot: root }), /bounded/);
  rmSync(file(root));
  const target = join(root, 'target');
  mkdirSync(target);
  rmSync(join(root, '.roster', 'asks'), { recursive: true });
  symlinkSync(target, join(root, '.roster', 'asks'), process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(saveClarification(vague, { repoRoot: root }), /symlinks/);
});

test('malformed schema, invalid UTF-8, hardlinks and POSIX public permissions cannot authorize continuation', async (t) => {
  const root = fixture(t);
  const record = await saveClarification(vague, { repoRoot: root });
  writeFileSync(file(root), JSON.stringify({ ...record, questions: ['Changed question'] }));
  await assert.rejects(resumeClarification(answer, { repoRoot: root }), /Invalid clarification/);
  writeFileSync(file(root), Buffer.from([0xff]));
  await assert.rejects(readClarification({ repoRoot: root }), /Invalid clarification/);
  rmSync(file(root));
  await saveClarification(vague, { repoRoot: root });
  linkSync(file(root), join(root, 'linked'));
  await assert.rejects(readClarification({ repoRoot: root }), /single-link/);
  await assert.rejects(cancelClarification({ repoRoot: root }), /single-link/);
  rmSync(join(root, 'linked'));
  if (process.platform !== 'win32') {
    chmodSync(file(root), 0o644);
    await assert.rejects(readClarification({ repoRoot: root }), /private/);
  }
});

test('a real shell process can inspect pending intake after restart and cancel with no model/test/publication requests', async (t) => {
  const root = fixture(t);
  await saveClarification(vague, { repoRoot: root, env: {} });
  const input = new PassThrough();
  const output = new PassThrough();
  let text = '';
  output.on('data', (chunk) => { text += chunk; });
  const done = startRepl({ input, output, errorOutput: output, cwd: root, config, env: {}, tty: false,
    historyStore: { load: async () => [], record: async () => {}, flush: async () => {} },
    services: { repositoryBranch: () => 'main', runBuiltinAsk: () => assert.fail('No seat should run') } });
  input.end('/clarify\n/clarify cancel\n/quit\n');
  assert.equal(await done, 0);
  assert.match(text, /Pending clarification:[\s\S]*Assumptions[\s\S]*Clarification cancelled/);
  assert.equal(existsSync(file(root)), false);
});

test('REPL streaming resume stages scope, keeps the checkpoint on quit, and requires approval after restart', async (t) => {
  const root = fixture(t);
  await saveClarification(vague, { repoRoot: root, env: {} });
  const followup = 'Write a short Status note. Output it in a new "notes" directory';
  async function session(lines, runBuiltinAsk) {
    const input = new PassThrough();
    const output = new PassThrough();
    let text = '';
    output.on('data', (chunk) => { text += chunk; });
    const done = startRepl({ input, output, errorOutput: output, cwd: root, config, env: {}, tty: false,
      historyStore: { load: async () => [], record: async () => {}, flush: async () => {} },
      services: { repositoryBranch: () => 'main', runBuiltinAsk } });
    input.end(lines);
    assert.equal(await done, 0, text);
    return text;
  }
  const paused = await session(`/clarify ${followup}\n/quit\n`, async (ask, options) => {
    assert.equal(options.confirm, true);
    assert.match(ask, /keep the public API unchanged[\s\S]*notes/);
    assert.equal((await readClarification({ repoRoot: root, env: {} })).answer, followup);
    return { askKind: 'slice', confirmedPause: true, planningOnly: true, failed: false,
      worktreePath: root, task: 'local-intake' };
  });
  assert.match(paused, /Paused by --confirm/);
  assert.ok(existsSync(file(root)));
  let calls = 0;
  const completed = await session('/clarify continue\n\n/quit\n', async (_ask, options) => {
    calls++;
    if (options.confirm) return { askKind: 'slice', confirmedPause: true, planningOnly: true, failed: false,
      worktreePath: root, task: 'local-intake' };
    assert.equal(options.confirm, false);
    assert.equal(options.preparedRun.worktreePath, root);
    assert.equal(options.publish, false);
    return { askKind: 'slice', failed: false };
  });
  assert.equal(calls, 2);
  assert.match(completed, /Clarification resumed/);
  assert.equal(existsSync(file(root)), false);
});

test('cancelling prepared clarification revokes Enter and retry authorization', async (t) => {
  const root = fixture(t);
  await saveClarification(vague, { repoRoot: root, env: {} });
  let calls = 0;
  const paused = shell(root, { runBuiltinAsk: async () => {
    calls++;
    return { askKind: 'slice', failed: false, planningOnly: true, confirmedPause: true,
      worktreePath: root, task: 'local-intake' };
  } });
  await paused.dispatch(`/clarify ${answer}`);
  assert.equal(calls, 1);
  await paused.dispatch('/clarify cancel');
  await paused.dispatch('');
  await assert.rejects(paused.dispatch('/retry'), /cancelled or changed/);
  assert.equal(calls, 1);
});
