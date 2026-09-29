import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { resolveContractsPath } from '../src/lib/paths.mjs';

const repoRoot = fileURLToPath(new URL('../', import.meta.url));

function git(cwd, ...args) {
  return execFileSync('git', [
    '-c', 'user.name=Trailer Test',
    '-c', 'user.email=trailer-test@example.test',
    '-c', 'commit.gpgsign=false',
    ...args,
  ], { cwd, encoding: 'utf8' }).trim();
}

test('the composite action runs the pinned PR checker on the complete commit range', (t) => {
  const actionDir = join(repoRoot, '.github', 'actions', 'check-agent-trailers');
  const action = readFileSync(join(actionDir, 'action.yml'), 'utf8');
  const invocation = /^\s+run: node "\$ACTION_PATH\/([^"\r\n]+)"$/m.exec(action);
  assert.ok(invocation, 'the action must run a checker relative to its own path');
  assert.match(action, /^\s+BASE_SHA: \$\{\{ inputs\.base-sha \}\}$/m);
  assert.match(action, /^\s+HEAD_SHA: \$\{\{ inputs\.head-sha \}\}$/m);

  const contracts = resolveContractsPath({ repoRoot });
  assert.equal(contracts, join(repoRoot, 'vendor', 'github-agent-contracts'));
  const checker = resolve(actionDir, invocation[1]);
  assert.equal(checker, join(contracts, 'scripts', 'check-pr-agent-trailers.mjs'));

  const cwd = mkdtempSync(join(tmpdir(), 'roster-trailer-ci-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  git(cwd, 'init', '--quiet');
  git(cwd, 'commit', '--quiet', '--allow-empty', '-m', 'base');
  const baseSha = git(cwd, 'rev-parse', 'HEAD');
  git(cwd, 'commit', '--quiet', '--allow-empty', '-m', 'valid', '-m', 'AI-Agent: fixture\nAI-Model: test');
  const validSha = git(cwd, 'rev-parse', 'HEAD');

  const runChecker = (headSha) => spawnSync(process.execPath, [checker], {
    cwd,
    encoding: 'utf8',
    env: { ...process.env, BASE_SHA: baseSha, HEAD_SHA: headSha, REQUIRE_ON_ALL_COMMITS: 'false' },
  });
  const valid = runChecker(validSha);
  assert.ifError(valid.error);
  assert.equal(valid.status, 0, valid.stderr);
  assert.equal(valid.stdout, `${validSha}: OK\n`);

  git(cwd, 'commit', '--quiet', '--allow-empty', '-m', 'invalid', '-m', 'AI-Agent: fixture');
  const invalidSha = git(cwd, 'rev-parse', 'HEAD');
  const invalid = runChecker(invalidSha);
  assert.ifError(invalid.error);
  assert.equal(invalid.status, 1, invalid.stderr);
  assert.equal(invalid.stdout, `${validSha}: OK\n${invalidSha}: missing trailers: AI-Model\n`);
});
