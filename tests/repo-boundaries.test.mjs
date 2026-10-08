import assert from 'node:assert/strict';
import { execFileSync, execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import test from 'node:test';
import {
  checkGitignoreCoverage,
  checkStagedRuntimeState,
  checkRosterScope,
} from '../scripts/check-roster-scope.mjs';
import {
  ensurePrivateFilesIgnored,
  readPrivateFile,
  writePrivateDocuments,
} from '../src/lib/private-files.mjs';
import {
  LEGACY_STATE_DIRNAME,
  openRepoState,
  readScopedState,
  writeScopedState,
  STATE_SCOPES,
} from '../src/lib/repo-state.mjs';
import {
  identityHash,
  compareIdentity,
} from '../src/lib/repo-identity.mjs';
import { resolveStateRoot, resolveMachineRoot } from '../src/lib/paths.mjs';
import { resolveSecret } from '../src/lib/secrets.mjs';
import { redactSecrets } from '../src/runtime/memory.mjs';

const runGit = promisify(execFile);

// Obvious non-credential sentinel: never a real or credential-shaped value.
const SENTINEL_KEY = 'test-only-private-api-key';

function tmp(name) {
  return mkdtempSync(join(tmpdir(), name));
}

function git(repo, ...args) {
  return runGit('git', args, { cwd: repo, encoding: 'utf8', timeout: 15_000 });
}

async function initRepo(root, { remote = null } = {}) {
  await git(root, 'init', '-b', 'main');
  await git(root, 'config', 'user.email', 'agent@example.test');
  await git(root, 'config', 'user.name', 'Agent');
  if (remote) await git(root, 'remote', 'add', 'origin', remote);
  writeFileSync(join(root, 'source-file.txt'), 'source of truth\n');
  await git(root, 'add', 'source-file.txt');
  await git(root, 'commit', '-m', 'baseline');
}

test('.gitignore covers all runtime state paths while tracked templates/docs stay tracked', async (t) => {
  const root = tmp('roster-gitignore-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, '.github', 'workflows'), { recursive: true });
  writeFileSync(join(root, '.github', 'workflows', 'template.yml'), 'name: template\n');
  mkdirSync(join(root, 'docs'), { recursive: true });
  writeFileSync(join(root, 'docs', 'TEMPLATE.md'), '# template\n');
  await initRepo(root, { remote: 'https://example.invalid/owner/a.git' });
  await git(root, 'add', '.github', 'docs');
  await git(root, 'commit', '-m', 'templates');

  // Every runtime path that must stay untracked (a subset of the guard list),
  // plus private setup files that are ignored by default until a contributor
  // deliberately tracks shared setup.
  const runtimePaths = [
    `${LEGACY_STATE_DIRNAME}/runs/`,
    `${LEGACY_STATE_DIRNAME}/logs/`,
    `${LEGACY_STATE_DIRNAME}/history`,
    `${LEGACY_STATE_DIRNAME}/checkpoints/`,
    `${LEGACY_STATE_DIRNAME}/locks/`,
    `${LEGACY_STATE_DIRNAME}/locks/issue-7.lock`,
    `${LEGACY_STATE_DIRNAME}/map.md`,
    `${LEGACY_STATE_DIRNAME}/config.yml`,
    `${LEGACY_STATE_DIRNAME}/fleet.yml`,
    `${LEGACY_STATE_DIRNAME}/capabilities.yml`,
    `${LEGACY_STATE_DIRNAME}/memory/`,
    `${LEGACY_STATE_DIRNAME}/asks/`,
    '.worktrees/',
    '.roster-state/',
  ];

  // The shipped .gitignore must already cover the runtime state (the guard
  // enforces this); copy it in and verify each path is git-ignored.
  const shippedIgnore = await fs.readFile(new URL('../.gitignore', import.meta.url), 'utf8');
  await fs.writeFile(join(root, '.gitignore'), shippedIgnore, 'utf8');

  // One git spawn for every path: non-matching paths print with a `::` source.
  const sharedSetup = `${LEGACY_STATE_DIRNAME}/README.md`;
  const { stdout: verdicts } = await git(root, 'check-ignore', '--no-index', '--verbose',
    '--non-matching', '--', ...runtimePaths, sharedSetup);
  const unignored = new Set(verdicts.split('\n').filter((line) => line.startsWith('::'))
    .map((line) => line.split('\t').pop()));
  for (const pattern of runtimePaths) {
    assert.ok(!unignored.has(pattern), `runtime path ${pattern} is not covered by .gitignore`);
  }
  // No blanket `.roster/` rule: shared setup a repo commits there stays trackable.
  assert.ok(unignored.has(sharedSetup), 'non-runtime .roster files are not ignored');

  // Tracked templates and docs remain tracked even with ignore rules present.
  const { stdout: tracked } = await git(root, 'ls-files');
  assert.ok(tracked.includes('.github/workflows/template.yml'), 'workflow template stays tracked');
  assert.ok(tracked.includes('docs/TEMPLATE.md'), 'docs template stays tracked');

  await git(root, 'add', '.gitignore');
  await git(root, 'commit', '-m', 'runtime state ignore rules');

  // Plant runtime state and confirm git sees nothing new to add.
  mkdirSync(join(root, LEGACY_STATE_DIRNAME, 'runs'), { recursive: true });
  writeFileSync(join(root, LEGACY_STATE_DIRNAME, 'runs', 'run.log'), 'log\n');
  mkdirSync(join(root, LEGACY_STATE_DIRNAME, 'locks'), { recursive: true });
  writeFileSync(join(root, LEGACY_STATE_DIRNAME, 'locks', 'issue-7.lock'), '{"holder":"seat"}\n');
  writeFileSync(join(root, LEGACY_STATE_DIRNAME, 'history'), 'history\n');
  writeFileSync(join(root, LEGACY_STATE_DIRNAME, 'config.yml'), 'llm: {}\n');
  const { stdout: staged } = await git(root, 'add', '-A', '--dry-run');
  assert.equal(staged, '', 'git add -A stages nothing while runtime state exists');
});

test('a staged runtime state file fails the commit-scope check', async (t) => {
  const root = tmp('roster-staged-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  await initRepo(root, { remote: 'https://example.invalid/owner/b.git' });
  const shippedIgnore = await fs.readFile(new URL('../.gitignore', import.meta.url), 'utf8');
  await fs.writeFile(join(root, '.gitignore'), shippedIgnore, 'utf8');

  // A runtime lock sneaks into the index (simulating a forced add).
  const lockFile = `${LEGACY_STATE_DIRNAME}/locks/issue-7.lock`;
  mkdirSync(join(root, LEGACY_STATE_DIRNAME, 'locks'), { recursive: true });
  writeFileSync(join(root, lockFile), '{"holder":"seat","pid":1}\n');
  execFileSync('git', ['add', '--force', lockFile], { cwd: root });
  const { stdout: staged } = await git(root, 'ls-files', '--', `${LEGACY_STATE_DIRNAME}/`);
  assert.ok(staged.includes('issue-7.lock'), 'precondition: runtime lock is staged');

  // The guard must fail on the staged lock, and pass once it is untracked.
  await assert.rejects(checkStagedRuntimeState({ repoRoot: root }), (error) => {
    assert.match(error.message, /runtime state/i);
    assert.deepEqual(error.paths, [lockFile]);
    return true;
  });
  await git(root, 'rm', '--cached', '--', lockFile);
  await assert.doesNotReject(checkStagedRuntimeState({ repoRoot: root }));
});

test('tracked contributor setup under .roster passes the commit-scope guard', async (t) => {
  const root = tmp('roster-setup-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  await initRepo(root, { remote: 'https://example.invalid/owner/setup.git' });
  const shippedIgnore = await fs.readFile(new URL('../.gitignore', import.meta.url), 'utf8');
  await fs.writeFile(join(root, '.gitignore'), shippedIgnore, 'utf8');

  // Shared contributor setup a repository chooses to commit. Config and fleet
  // are ignored by default for private local copies, so tracking them is a
  // deliberate forced add; once tracked they must never be denied.
  const setup = {
    [`${LEGACY_STATE_DIRNAME}/config.yml`]: 'llm:\n  api_key_env: ROSTER_TEST_SECRET\n',
    [`${LEGACY_STATE_DIRNAME}/fleet.yml`]: 'profiles: {}\n',
    [`${LEGACY_STATE_DIRNAME}/capabilities.yml`]: 'seats: {}\n',
    [`${LEGACY_STATE_DIRNAME}/skills/review/SKILL.md`]: '# review\n',
    [`${LEGACY_STATE_DIRNAME}/recipes/default.yml`]: 'seats: [planner, coder, reviewer]\n',
    [`${LEGACY_STATE_DIRNAME}/docs/SETUP.md`]: '# setup\n',
  };
  for (const [file, source] of Object.entries(setup)) {
    mkdirSync(join(root, file, '..'), { recursive: true });
    writeFileSync(join(root, file), source);
  }
  await git(root, 'add', '--force', '--', ...Object.keys(setup));
  await git(root, 'add', '.gitignore');
  await git(root, 'commit', '-m', 'shared roster setup');

  const result = await checkRosterScope({ repoRoot: root });
  assert.equal(result.ok, true, 'tracked shared setup is not runtime state');
  const { stdout: tracked } = await git(root, 'ls-files', '--', `${LEGACY_STATE_DIRNAME}/`);
  assert.deepEqual(tracked.trim().split('\n').sort(), Object.keys(setup).sort(),
    'every shared setup file stays tracked');

  // Runtime state beside that setup is still refused once staged.
  const lockFile = `${LEGACY_STATE_DIRNAME}/locks/issue-7.lock`;
  mkdirSync(join(root, LEGACY_STATE_DIRNAME, 'locks'), { recursive: true });
  writeFileSync(join(root, lockFile), '{"holder":"seat"}\n');
  const { stdout: wouldAdd } = await git(root, 'add', '-A', '--dry-run');
  assert.equal(wouldAdd, '', 'a runtime lock is ignored next to tracked setup');
  await git(root, 'add', '--force', lockFile);
  await assert.rejects(checkRosterScope({ repoRoot: root }), (error) => {
    assert.deepEqual(error.paths, [lockFile]);
    return true;
  });
});

test('the shipped .gitignore passes the ignore-coverage guard', async () => {
  const result = await checkGitignoreCoverage({});
  assert.equal(result.ok, true);
});

test('runtime state files carry no secrets and never touch the vault', async (t) => {
  const machineRoot = tmp('roster-vault-');
  t.after(() => rmSync(machineRoot, { recursive: true, force: true }));
  mkdirSync(join(machineRoot, 'vault'), { recursive: true });
  // Vault fixture: an in-memory-style store with one sentinel secret, in the
  // shape the app's resolveSecret consumes ({ get, set }).
  const vaultStore = new Map([['ROSTER_TEST_SECRET', SENTINEL_KEY]]);
  const vault = {
    get: async (name) => vaultStore.get(name),
    set: async (name, value) => vaultStore.set(name, value),
  };
  const vaultFilesBefore = await fs.readdir(join(machineRoot, 'vault'));

  const root = tmp('roster-secrets-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  await initRepo(root, { remote: 'https://example.invalid/owner/c.git' });
  const shippedIgnore = await fs.readFile(new URL('../.gitignore', import.meta.url), 'utf8');
  await fs.writeFile(join(root, '.gitignore'), shippedIgnore, 'utf8');

  await ensurePrivateFilesIgnored(root, ['config.yml']);
  // Private config declares the secret by name only; the value stays in the vault.
  await writePrivateDocuments([
    { name: 'config.yml', source: 'llm:\n  api_key_env: ROSTER_TEST_SECRET\n' },
  ], { repoRoot: root });

  const configSource = await readPrivateFile(root, 'config.yml');
  assert.ok(configSource.includes('api_key_env: ROSTER_TEST_SECRET'));
  assert.ok(!configSource.includes(SENTINEL_KEY), 'vault secret value never written into repo state');

  // Vault round-trip works; repo-state writes never move vault entries.
  assert.equal(await vault.get('ROSTER_TEST_SECRET'), SENTINEL_KEY);
  // Resolve the sentinel *through the app code* and push it into a runtime
  // state record the same way a logger would: redaction drops it from any
  // text that is about to be persisted into repo state.
  const resolved = await resolveSecret('ROSTER_TEST_SECRET', { env: {}, vault });
  assert.equal(resolved, SENTINEL_KEY, 'app secret resolution returns the vault sentinel');
  const record = { time: '2026-01-01T00:00:00Z', issue: null, task: 'issue-1', session: 's',
    changed: 'No code changes.', tests: 'Not run.', next_gap: 'none',
    status: 'ok', summary: `key=${SENTINEL_KEY} used` };
  const redactedSummary = redactSecrets(record.summary, { env: { ROSTER_TEST_SECRET: resolved } });
  const persisted = JSON.stringify({ ...record, summary: redactedSummary });
  mkdirSync(join(root, LEGACY_STATE_DIRNAME, 'memory'), { recursive: true });
  writeFileSync(join(root, LEGACY_STATE_DIRNAME, 'memory', 'coder.jsonl'), `${persisted}\n`);
  const stateContents = readFileSync(join(root, LEGACY_STATE_DIRNAME, 'memory', 'coder.jsonl'), 'utf8');
  assert.ok(!stateContents.includes(SENTINEL_KEY),
    'runtime state record written by the app carries no vault secret');
  assert.ok(stateContents.includes('[redacted]'), 'the sentinel was redacted at the app boundary');

  const vaultFilesAfter = await fs.readdir(join(machineRoot, 'vault'));
  assert.deepEqual(vaultFilesAfter, vaultFilesBefore, 'no vault content moves into or out of repo state');

  // The state file itself carries no credential-shaped content.
  const { stdout } = await git(root, 'ls-files');
  assert.ok(!stdout.includes(SENTINEL_KEY), 'no tracked file contains the sentinel secret');
});

test('deleting .roster leaves vault, sources, machine history, and published evidence untouched', async (t) => {
  const machineRoot = tmp('roster-delete-');
  t.after(() => rmSync(machineRoot, { recursive: true, force: true }));
  mkdirSync(join(machineRoot, 'vault'), { recursive: true });
  const vaultStore = new Map([['ROSTER_DELETE_SECRET', SENTINEL_KEY]]);
  const vault = {
    get: async (name) => vaultStore.get(name),
    set: async (name, value) => vaultStore.set(name, value),
  };
  writeFileSync(join(machineRoot, 'vault', 'store.json'),
    JSON.stringify(Object.fromEntries(vaultStore)), 'utf8');

  const root = tmp('roster-delete-repo-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  await initRepo(root, { remote: 'https://example.invalid/owner/d.git' });
  const shippedIgnore = await fs.readFile(new URL('../.gitignore', import.meta.url), 'utf8');
  await fs.writeFile(join(root, '.gitignore'), shippedIgnore, 'utf8');
  await ensurePrivateFilesIgnored(root, ['config.yml']);
  await writePrivateDocuments([{ name: 'config.yml', source: 'llm: {}\n' }], { repoRoot: root });

  // Machine history lives under the machine root, keyed by repo identity.
  const historyDir = join(machineRoot, 'history', 'github.com--example--owner-d');
  mkdirSync(historyDir, { recursive: true });
  writeFileSync(join(historyDir, 'index.ndjson'), '{"schema_version":1,"event":"run"}\n');

  // Published GitHub evidence lives in the repo as tracked files.
  writeFileSync(join(root, 'agent-policy.yml'), 'policy: reviewed\n');
  await git(root, 'add', 'agent-policy.yml');
  await git(root, 'commit', '-m', 'published evidence');

  // Deleting .roster removes only repo state.
  rmSync(join(root, LEGACY_STATE_DIRNAME), { recursive: true, force: true });
  assert.ok(!statSync(join(root, LEGACY_STATE_DIRNAME), { throwIfNoEntry: false }), '.roster is gone');

  assert.equal(await vault.get('ROSTER_DELETE_SECRET'), SENTINEL_KEY, 'vault secrets survive');
  assert.equal(readFileSync(join(root, 'source-file.txt'), 'utf8'), 'source of truth\n');
  assert.equal(readFileSync(join(historyDir, 'index.ndjson'), 'utf8'),
    '{"schema_version":1,"event":"run"}\n', 'machine history survives');
  const { stdout } = await git(root, 'log', '--oneline', '--', 'agent-policy.yml');
  assert.ok(stdout.length > 0, 'published GitHub evidence remains tracked');
});

test('recloning the checkout keeps prior machine history readable', async (t) => {
  const machineRoot = tmp('roster-reclone-');
  t.after(() => rmSync(machineRoot, { recursive: true, force: true }));
  const env = { ...process.env, PATHS_OVERRIDE: machineRoot };

  const remoteA = 'https://github.invalid/acme/roster-app.git';
  const cloneA = tmp('roster-reclone-a-');
  const cloneB = tmp('roster-reclone-b-');
  t.after(() => {
    rmSync(cloneA, { recursive: true, force: true });
    rmSync(cloneB, { recursive: true, force: true });
  });
  await initRepo(cloneA, { remote: remoteA });
  await initRepo(cloneB, { remote: remoteA });

  // Two checkouts of the same remote URL carry a different local directory
  // name in each. The durable identity (remote URL plus repo path) must
  // distinguish them even when the slug collides.
  const hashA = identityHash({ gitCommonDir: join(cloneA, '.git'), remoteUrl: remoteA });
  const hashB = identityHash({ gitCommonDir: join(cloneB, '.git'), remoteUrl: remoteA });
  assert.notEqual(hashA, hashB, 'same remote from two different checkouts still hashes distinctly');

  // An explicit identity token (the documented durable-identity escape hatch)
  // is stable across path changes: deleting and recloning at a new directory
  // preserves the same key, so machine history stays readable.
  const token = 'github.com--example--github-agent-roster';
  const idA = (await resolveStateRoot({ repoRoot: cloneA, env: { ...env, ROSTER_REPO_ID: token } })).repoId;
  rmSync(cloneA, { recursive: true, force: true });
  const idB = (await resolveStateRoot({ repoRoot: cloneB, env: { ...env, ROSTER_REPO_ID: token } })).repoId;
  assert.equal(idA, idB, 'the explicit durable identity survives reclone at a new path');

  // Write machine history under the shared machine root for that identity.
  const machineA = resolveMachineRoot({ env });
  const historyDir = join(machineA.root, 'history', idA);
  mkdirSync(historyDir, { recursive: true });
  writeFileSync(join(historyDir, 'index.ndjson'), '{"schema_version":1,"event":"run"}\n');

  // Reopen through the surviving checkout: same identity, readable history.
  const handleB = await resolveStateRoot({ repoRoot: cloneB, env: { ...env, ROSTER_REPO_ID: token } });
  assert.equal(handleB.repoId, idA, 'recloned checkout resolves the same durable identity');
  assert.equal(readFileSync(join(historyDir, 'index.ndjson'), 'utf8'),
    '{"schema_version":1,"event":"run"}\n', 'prior machine history stays readable');
});

test('two similar repos resolve distinct state roots and never share state', async (t) => {
  const machineRoot = tmp('roster-similar-');
  t.after(() => rmSync(machineRoot, { recursive: true, force: true }));
  const env = { ...process.env, PATHS_OVERRIDE: machineRoot };

  // Same name, different owner/remote: durable identity must differ.
  const remoteA = 'https://github.invalid/owner-a/similar-repo.git';
  const remoteB = 'https://github.invalid/owner-b/similar-repo.git';
  const rootA = tmp('roster-similar-a-');
  const rootB = tmp('roster-similar-b-');
  t.after(() => {
    rmSync(rootA, { recursive: true, force: true });
    rmSync(rootB, { recursive: true, force: true });
  });
  await initRepo(rootA, { remote: remoteA });
  await initRepo(rootB, { remote: remoteB });

  const handleA = await resolveStateRoot({ repoRoot: rootA, env });
  const handleB = await resolveStateRoot({ repoRoot: rootB, env });
  assert.notEqual(handleA.root, handleB.root, 'similar repos with different remotes get distinct state roots');
  assert.notEqual(handleA.repoId, handleB.repoId, 'similar repos with different remotes get distinct repo ids');

  // A third checkout with a similar remote (same name, yet another owner)
  // must land in yet another state root — similar names never share state.
  const remoteC = 'https://github.invalid/owner-c/similar-repo.git';
  const rootC = tmp('roster-similar-c-');
  t.after(() => rmSync(rootC, { recursive: true, force: true }));
  await initRepo(rootC, { remote: remoteC });
  const handleC = await resolveStateRoot({ repoRoot: rootC, env });
  assert.notEqual(handleC.repoId, handleA.repoId, 'similar remote C never shares identity with A');

  // Identity hashes from git metadata stay distinct per remote as well.
  const hashA = identityHash({ gitCommonDir: join(rootA, '.git'), remoteUrl: remoteA });
  const hashB = identityHash({ gitCommonDir: join(rootB, '.git'), remoteUrl: remoteB });
  assert.notEqual(hashA, hashB, 'repo identity hashes differ for different remotes');
  assert.equal(compareIdentity(hashA, hashA).status, 'match');
  assert.equal(compareIdentity(hashA, hashB).status, 'mismatch');

  // Scoped state writes never cross the boundary.
  await writeScopedState('{"value":"repo-a"}', { scope: STATE_SCOPES.SHARED, repoRoot: rootA });
  const fromB = await readScopedState({ scope: STATE_SCOPES.SHARED, repoRoot: rootB });
  assert.equal(fromB, null, 'repo B never reads repo A state');
  const fromA = await readScopedState({ scope: STATE_SCOPES.SHARED, repoRoot: rootA });
  assert.equal(fromA, '{"value":"repo-a"}');
});

test('a fresh clone starts with empty repo state and full functionality', async (t) => {
  const machineRoot = tmp('roster-fresh-');
  t.after(() => rmSync(machineRoot, { recursive: true, force: true }));
  const env = { ...process.env, PATHS_OVERRIDE: machineRoot };
  const root = tmp('roster-fresh-repo-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  await initRepo(root, { remote: 'https://example.invalid/owner/f.git' });

  const handle = await resolveStateRoot({ repoRoot: root, env });
  // The state root does not exist yet: a fresh clone has no repo state.
  const opened = await openRepoState(handle.root, { fileSystem: fs });
  assert.equal(opened.initialized, true, 'fresh state initializes on first open');
  assert.deepEqual(opened.state, {}, 'fresh state is empty');

  // Empty-state contract: reads are null (not errors), writes work, re-reads match.
  const key = ['state', 'repo.json'];
  assert.equal(await readScopedState({ scope: STATE_SCOPES.SHARED, repoRoot: root, segments: key }), null);
  await writeScopedState(JSON.stringify({ schema_version: 1, tasks: [] }), {
    scope: STATE_SCOPES.SHARED, repoRoot: root, segments: key,
  });
  const readBack = await readScopedState({ scope: STATE_SCOPES.SHARED, repoRoot: root, segments: key });
  assert.equal(readBack, JSON.stringify({ schema_version: 1, tasks: [] }));
});

test('the full commit-scope guard passes on a clean worktree', async (t) => {
  const root = tmp('roster-scope-');
  t.after(() => rmSync(root, { recursive: true, force: true }));
  await initRepo(root, { remote: 'https://example.invalid/owner/g.git' });
  const shippedIgnore = await fs.readFile(new URL('../.gitignore', import.meta.url), 'utf8');
  await fs.writeFile(join(root, '.gitignore'), shippedIgnore, 'utf8');
  const result = await checkRosterScope({ repoRoot: root });
  assert.equal(result.ok, true);
});
