import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { formatInit, initializeRoster } from '../src/lib/init.mjs';

const installationRoot = fileURLToPath(new URL('../', import.meta.url));

function fixture(t) {
  const cwd = mkdtempSync(join(tmpdir(), 'roster-init-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  writeFileSync(join(cwd, 'agent-policy.yml'), 'human-owned-policy\n');
  return cwd;
}

test('init copies examples without creating or replacing human policy or existing examples', async (t) => {
  const cwd = fixture(t);
  const created = await initializeRoster({ cwd, installationRoot });
  assert.deepEqual(created, [
    { path: join(cwd, 'roster.config.example.yml'), status: 'created' },
    { path: join(cwd, 'ROSTER-POLICY-NOTE.md'), status: 'created' },
  ]);
  assert.equal(readFileSync(join(cwd, 'roster.config.example.yml'), 'utf8'),
    readFileSync(join(installationRoot, 'roster.config.example.yml'), 'utf8'));
  assert.equal(readFileSync(join(cwd, 'ROSTER-POLICY-NOTE.md'), 'utf8'),
    readFileSync(join(installationRoot, 'templates', 'init', 'POLICY-NOTE.md'), 'utf8'));
  assert.equal(readFileSync(join(cwd, 'agent-policy.yml'), 'utf8'), 'human-owned-policy\n');
  writeFileSync(join(cwd, 'roster.config.example.yml'), 'custom config\n');
  writeFileSync(join(cwd, 'ROSTER-POLICY-NOTE.md'), 'custom note\n');
  assert.deepEqual((await initializeRoster({ cwd, installationRoot })).map(({ status }) => status),
    ['kept', 'kept']);
  assert.equal(readFileSync(join(cwd, 'roster.config.example.yml'), 'utf8'), 'custom config\n');
  assert.equal(readFileSync(join(cwd, 'ROSTER-POLICY-NOTE.md'), 'utf8'), 'custom note\n');
  assert.equal(readFileSync(join(cwd, 'agent-policy.yml'), 'utf8'), 'human-owned-policy\n');
  assert.match(formatInit(created), /Policy: agent-policy\.yml unchanged \(human-owned\)/);
});

test('init refuses a directory at an example destination rather than overwriting it', async (t) => {
  const cwd = fixture(t);
  mkdirSync(join(cwd, 'roster.config.example.yml'));
  await assert.rejects(initializeRoster({ cwd, installationRoot }), /Refusing to replace/);
  assert.equal(readFileSync(join(cwd, 'agent-policy.yml'), 'utf8'), 'human-owned-policy\n');
});

test('init CLI writes examples in the caller directory without network or policy changes', (t) => {
  const cwd = fixture(t);
  const cli = join(installationRoot, 'src', 'cli.mjs');
  const result = spawnSync(process.execPath, [cli, 'init'], {
    cwd, encoding: 'utf8', timeout: 10_000,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Created: .*roster\.config\.example\.yml/);
  assert.match(result.stdout, /Created: .*ROSTER-POLICY-NOTE\.md/);
  assert.match(result.stdout, /agent-policy\.yml unchanged/);
  assert.equal(readFileSync(join(cwd, 'agent-policy.yml'), 'utf8'), 'human-owned-policy\n');
});
