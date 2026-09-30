import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { checkDoctor, formatDoctor } from '../src/lib/doctor.mjs';

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
  return { cwd, installationRoot, configFile,
    env: { GITHUB_APP_ID: 'secret-app-id', GITHUB_APP_PRIVATE_KEY_PATH: keyPath } };
}

test('doctor reports six offline prerequisites without exposing App values', (t) => {
  const paths = fixture(t);
  const result = checkDoctor({
    ...paths, nodeVersion: 'v20.19.0',
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
    ...paths, nodeVersion: 'v19.9.0',
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
    const result = checkDoctor(breakCheck(fixture(t)));
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
  const result = checkDoctor({ ...paths, env: {} });
  assert.equal(result.ok, true);
  assert.deepEqual(result.checks.map(({ skipped }) => Boolean(skipped)), [false, false, true, true, true, false]);
  assert.match(formatDoctor(result), /^SKIP agent-policy\.yml/m);
  writeFileSync(paths.configFile, example.replace('enabled: true', 'enabled: false')
    .replace('base_url: ""', 'base_url: http://localhost:8000/v1'));
  assert.equal(checkDoctor({ ...paths, env: {} }).checks[5].ok, false);
});

test('offline nonpublishing config may omit a model, while malformed private config fails explicitly', (t) => {
  const paths = fixture(t);
  writeFileSync(paths.configFile, example.replace('enabled: true', 'enabled: false'));
  const offline = checkDoctor({ ...paths, env: {} });
  assert.equal(offline.ok, true);
  assert.equal(offline.checks[5].skipped, true);
  writeFileSync(paths.configFile, 'not: a valid config\n');
  const malformed = checkDoctor({ ...paths, env: {} });
  assert.equal(malformed.ok, false);
  assert.match(formatDoctor(malformed), /config could not be read or validated/);
});
