import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { test } from 'node:test';
import { checkDoctor, formatDoctor, redactRoot } from '../src/lib/doctor.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'roster-doctor-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const installationRoot = join(root, 'install');
  const cwd = join(root, 'project');
  mkdirSync(join(installationRoot, 'vendor', 'github-agent-contracts', 'scripts'), { recursive: true });
  writeFileSync(join(installationRoot, 'vendor', 'github-agent-contracts',
    'scripts', 'agent-pr.mjs'), 'export {};\n');
  mkdirSync(join(cwd, '.github', 'workflows'), { recursive: true });
  writeFileSync(join(cwd, 'agent-policy.yml'), 'version: 1\n');
  writeFileSync(join(cwd, '.github', 'workflows', 'check-agent-trailers.yml'),
    'name: check-agent-trailers\n');
  writeFileSync(join(installationRoot, 'roster.config.example.yml'), example);
  mkdirSync(join(cwd, '.roster'));
  const configFile = join(cwd, '.roster', 'config.yml');
  writeFileSync(configFile, example.replace('model: ""', 'model: served-model'));
  const keyPath = join(root, 'secret-key-path.pem');
  writeFileSync(keyPath, 'fixture-only key marker\n');
  mkdirSync(join(root, 'state'));
  return { cwd, installationRoot, configFile,
    env: { GITHUB_APP_ID: 'secret-app-id', GITHUB_APP_PRIVATE_KEY_PATH: keyPath } };
}

test('doctor reports six offline prerequisites without exposing App values', (t) => {
  const paths = fixture(t);
  const result = checkDoctor({
    ...paths, nodeVersion: 'v20.19.0', stateRoots: false,
  });
  assert.equal(result.ok, true);
  assert.equal(result.checks.length, 6);
  assert.ok(result.checks.every((check) => check.ok));
  const output = formatDoctor(result);
  assert.match(output, /^OK Node\.js >=20/m);
  assert.match(output, /^OK contracts agent-pr\.mjs/m);
  assert.match(output, /^OK GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH present/m);
  assert.ok(!output.includes('secret-app-id'));
  assert.ok(!output.includes('secret-key-path'));
  assert.ok(!JSON.stringify(result).includes('secret-key-path'));
  assert.match(output, /^OK \.roster\/config\.yml model/m);
});

test('doctor marks absent or unsupported prerequisites as failures instead of printing values', (t) => {
  const paths = fixture(t);
  unlinkSync(join(paths.installationRoot, 'vendor', 'github-agent-contracts',
    'scripts', 'agent-pr.mjs'));
  unlinkSync(join(paths.cwd, 'agent-policy.yml'));
  unlinkSync(join(paths.cwd, '.github', 'workflows', 'check-agent-trailers.yml'));
  unlinkSync(paths.configFile);
  const result = checkDoctor({
    ...paths, nodeVersion: 'v19.9.0', stateRoots: false,
    env: { GITHUB_APP_ID: 'secret-app-id', GITHUB_APP_PRIVATE_KEY_PATH: '' },
  });
  assert.equal(result.ok, false);
  assert.deepEqual(result.checks.map(({ ok }) => ok), [false, false, false, false, false, false]);
  assert.equal((formatDoctor(result).match(/^FAIL /gm) ?? []).length, 6);
  assert.ok(!formatDoctor(result).includes('secret-app-id'));
});

for (const [name, index, breakCheck] of [
  ['Node version', 0, (paths) => ({ ...paths, nodeVersion: 'v19.9.0' })],
  ['vendor publisher', 1, (paths) => {
    unlinkSync(join(paths.installationRoot, 'vendor', 'github-agent-contracts', 'scripts', 'agent-pr.mjs'));
    return paths;
  }],
  ['App env', 2, (paths) => ({ ...paths, env: {} })],
  ['App PEM', 2, (paths) => {
    unlinkSync(paths.env.GITHUB_APP_PRIVATE_KEY_PATH);
    return paths;
  }],
  ['policy', 3, (paths) => { unlinkSync(join(paths.cwd, 'agent-policy.yml')); return paths; }],
  ['workflow', 4, (paths) => {
    unlinkSync(join(paths.cwd, '.github', 'workflows', 'check-agent-trailers.yml'));
    return paths;
  }],
  ['model', 5, (paths) => {
    writeFileSync(paths.configFile, example);
    return { ...paths, env: { ...paths.env, AI_MODEL: 'ghcp-model-is-not-the-vllm-model' } };
  }],
]) {
  test(`doctor detects ${name} failure independently`, (t) => {
    const result = checkDoctor({ stateRoots: false, ...breakCheck(fixture(t)) });
    assert.equal(result.ok, false);
    assert.deepEqual(result.checks.map(({ ok }) => ok),
      Array.from({ length: 6 }, (_, position) => position !== index));
  });
}

test('run-only onboarding skips App/policy/workflow but still checks the saved model', (t) => {
  const paths = fixture(t);
  writeFileSync(paths.configFile, example.replace('enabled: true', 'enabled: false')
    .replace('base_url: ""', 'base_url: http://localhost:8000/v1').replace('model: ""', 'model: served-model'));
  unlinkSync(join(paths.cwd, 'agent-policy.yml'));
  unlinkSync(join(paths.cwd, '.github', 'workflows', 'check-agent-trailers.yml'));
  const result = checkDoctor({ ...paths, env: {}, stateRoots: false });
  assert.equal(result.ok, true);
  assert.deepEqual(result.checks.map(({ skipped }) => Boolean(skipped)), [false, false, true, true, true, false]);
  assert.match(formatDoctor(result), /^SKIP agent-policy\.yml/m);
  writeFileSync(paths.configFile, example.replace('enabled: true', 'enabled: false')
    .replace('base_url: ""', 'base_url: http://localhost:8000/v1'));
  assert.equal(checkDoctor({ ...paths, env: {}, stateRoots: false }).checks[5].ok, false);
});

test('offline nonpublishing config may omit a model, while malformed private config fails explicitly', (t) => {
  const paths = fixture(t);
  writeFileSync(paths.configFile, example.replace('enabled: true', 'enabled: false'));
  const offline = checkDoctor({ ...paths, env: {}, stateRoots: false });
  assert.equal(offline.ok, true);
  assert.equal(offline.checks[5].skipped, true);
  writeFileSync(paths.configFile, 'not: a valid config\n');
  const malformed = checkDoctor({ ...paths, env: {}, stateRoots: false });
  assert.equal(malformed.ok, false);
  assert.match(formatDoctor(malformed), /config could not be read or validated/);
});

test('doctor reports machine, repository, and state roots with redacted paths', (t) => {
  const paths = fixture(t);
  const stateDir = join(dirname(paths.cwd), 'state');
  const result = checkDoctor({
    ...paths, stateRoots: true, env: { ...paths.env, ROSTER_STATE_ROOT: stateDir },
  });
  assert.equal(result.ok, true);
  assert.equal(result.checks.length, 9);
  const output = formatDoctor(result);
  assert.match(output, /^OK machine state root$/m);
  assert.match(output, /^OK repository root$/m);
  assert.match(output, /^OK repository state root$/m);
  // The rendered `roster doctor` output itself must show the resolved machine
  // and state roots, using redacted (tilde-abbreviated) display paths.
  assert.match(output, /^machine root: /m);
  assert.match(output, /^state root: /m);
  assert.ok(!output.includes(stateDir), 'full state root path must not be rendered');
  assert.ok(!output.includes(paths.cwd), 'full repository path must not be rendered');
  const serialized = JSON.stringify(result);
  assert.ok(!serialized.includes('test-only-private-api-key'), serialized);
  assert.ok(!serialized.includes('secret-app-id'), serialized);
  assert.ok(!serialized.includes('secret-key-path'), serialized);
  assert.ok(!serialized.includes(stateDir), 'full state root must be redacted');
  assert.match(result.roots.machine, /…/);
  assert.match(result.roots.state, /…/);
});

test('doctor output never echoes a credential-shaped sentinel passed through the module', (t) => {
  const paths = fixture(t);
  const stateDir = join(dirname(paths.cwd), 'state');
  const result = checkDoctor({
    ...paths, stateRoots: true, machineRoot: { root: stateDir, writable: true, platform: process.platform },
    env: { ...paths.env, ROSTER_STATE_ROOT: stateDir, ROSTER_API_KEY: 'test-only-private-api-key' },
  });
  const output = `${formatDoctor(result)}${JSON.stringify(result)}`;
  assert.ok(!output.includes('test-only-private-api-key'), output);
  assert.ok(!output.includes(stateDir), output);
  // The roots lines are present and redacted, not omitted.
  assert.match(output, /^machine root: /m);
  assert.match(output, /^state root: /m);
});

test('doctor reports state-root failure as a FAIL check instead of falling back silently', (t) => {
  const paths = fixture(t);
  const missingState = join(dirname(paths.cwd), 'not-a-state-root');
  const result = checkDoctor({
    ...paths, stateRoots: true, env: { ...paths.env, ROSTER_STATE_ROOT: missingState },
  });
  assert.equal(result.ok, false);
  assert.match(formatDoctor(result), /^FAIL repository state root/m);
  assert.equal(result.roots.state, '<unknown>');
});

test('redactRoot tilde-abbreviates private display paths', () => {
  const home = process.platform === 'win32' ? 'C:\\Users\\me' : '/Users/me';
  const joiner = process.platform === 'win32' ? '\\' : '/';
  // Home-relative paths collapse to ~<sep>…<sep><last-segment>, exactly the
  // display form TASK.md requires: private prefixes never reach the output.
  assert.equal(redactRoot(join(home, 'state', 'roster'), { home }), `~${joiner}…${joiner}roster`);
  assert.equal(redactRoot(join(home, 'state', 'roster', 'state'), { home }), `~${joiner}…${joiner}state`);
  assert.equal(redactRoot(join(home, 'roster'), { home }), `~${joiner}roster`);
  assert.equal(redactRoot('/var/tmp/state', { home: '/Users/me' }), 'var…state');
  assert.equal(redactRoot('C:\\Users\\me\\AppData\\Local\\roster\\state', { home: 'C:\\Users\\me' }),
      '~\\…\\state');
  assert.equal(redactRoot('relative'), 'relative');
  assert.equal(redactRoot(''), '<unknown>');
});

test('formatDoctor renders per-item health plus machine/state roots for roster doctor', () => {
  const base = mkdtempSync(join(tmpdir(), 'roster-format-'));
  rmSync(base, { recursive: true, force: true });
  const result = {
    ok: true,
    checks: [
      { name: 'machine state root', ok: true },
      { name: 'repository root', ok: true },
      { name: 'repository state root', ok: true },
      { name: 'Node.js >=20', ok: true },
      { name: 'contracts agent-pr.mjs', ok: true },
      { name: '.roster/config.yml model', ok: true },
    ],
    roots: {
      machine: join(base, 'machine-root'),
      repository: join(base, 'repo-root'),
      state: join(base, 'machine-root', 'repos', 'state'),
      scope: 'repository',
      repoId: 'repo-0123456789abcdef',
    },
  };
  const output = formatDoctor(result);
  // The required `roster doctor` lines are rendered, not just carried on the
  // result object, and they are tilde-abbreviated rather than verbatim.
  assert.match(output, /^machine root: .+/m);
  assert.match(output, /^state root: .+/m);
  assert.ok(!output.includes(join(base, 'machine-root')), 'full machine root must not render');
  assert.ok(!output.includes(join(base, 'repo-root')), 'full repository root must not render');
  // Credentials passed through the roots object never reach the rendered output.
  const leaking = {
    ...result,
    roots: { ...result.roots, machine: `${base} api_key=test-only-private-api-key` },
  };
  assert.ok(!formatDoctor(leaking).includes('test-only-private-api-key'));
});

