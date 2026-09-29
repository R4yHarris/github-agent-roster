import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { checkDoctor, formatDoctor } from '../src/lib/doctor.mjs';

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
  return { cwd, installationRoot };
}

test('doctor reports exactly five offline prerequisites without exposing App values', (t) => {
  const paths = fixture(t);
  const result = checkDoctor({
    ...paths, nodeVersion: 'v20.19.0',
    env: { GITHUB_APP_ID: 'secret-app-id', GITHUB_APP_PRIVATE_KEY_PATH: 'secret-key-path' },
  });
  assert.equal(result.ok, true);
  assert.equal(result.checks.length, 5);
  assert.ok(result.checks.every((check) => check.ok));
  const output = formatDoctor(result);
  assert.match(output, /^OK Node\.js >=20/m);
  assert.match(output, /^OK contracts agent-pr\.mjs/m);
  assert.match(output, /^OK GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH present/m);
  assert.ok(!output.includes('secret-app-id'));
  assert.ok(!output.includes('secret-key-path'));
  assert.ok(!JSON.stringify(result).includes('secret-key-path'));
});

test('doctor marks absent or unsupported prerequisites as failures instead of printing values', (t) => {
  const paths = fixture(t);
  unlinkSync(join(paths.installationRoot, 'vendor', 'github-agent-contracts',
    'scripts', 'agent-pr.mjs'));
  unlinkSync(join(paths.cwd, 'agent-policy.yml'));
  unlinkSync(join(paths.cwd, '.github', 'workflows', 'check-agent-trailers.yml'));
  const result = checkDoctor({
    ...paths, nodeVersion: 'v19.9.0',
    env: { GITHUB_APP_ID: 'secret-app-id', GITHUB_APP_PRIVATE_KEY_PATH: '' },
  });
  assert.equal(result.ok, false);
  assert.deepEqual(result.checks.map(({ ok }) => ok), [false, false, false, false, false]);
  assert.equal((formatDoctor(result).match(/^FAIL /gm) ?? []).length, 5);
  assert.ok(!formatDoctor(result).includes('secret-app-id'));
});
