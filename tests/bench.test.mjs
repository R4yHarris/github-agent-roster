import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const cli = path.join(root, 'src', 'cli.mjs');
const expectedFields = [
  'process_start_ms',
  'config_load_ms',
  'command_dispatch_ms',
  'git_worktree_add_ms',
  'submodule_init_ms',
  'mocked_model_round_trip_ms',
];

test('roster help starts with Node and no installed repository files', (t) => {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'roster-empty-install-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const isolatedCli = path.join(directory, 'cli.mjs');
  copyFileSync(cli, isolatedCli);
  const result = spawnSync(process.execPath, [isolatedCli, '--help'], {
    cwd: directory, encoding: 'utf8', timeout: 10_000,
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /roster bench/);
});

test('offline bench writes exactly six numeric, secret-free timings and checks dispatch budget', (t) => {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'roster-bench-report-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const result = spawnSync(process.execPath, [cli, 'bench'], {
    cwd: directory, encoding: 'utf8', timeout: 30_000,
    env: { ...process.env, ROSTER_API_KEY: 'bench-secret-token',
      GITHUB_APP_PRIVATE_KEY_PATH: 'private-key-path.pem' },
  });
  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(readFileSync(path.join(directory, '.roster', 'bench.json'), 'utf8'));
  assert.deepEqual(Object.keys(report), expectedFields);
  for (const value of Object.values(report)) {
    assert.equal(Number.isFinite(value) && value >= 0, true);
  }
  assert.ok(report.command_dispatch_ms <= 150, `dispatch took ${report.command_dispatch_ms}ms`);
  assert.doesNotMatch(JSON.stringify(report), /bench-secret-token|private-key-path|[A-Za-z]:\\/);
  assert.deepEqual(JSON.parse(result.stdout), report);
});
