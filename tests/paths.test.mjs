import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { relative, resolve, sep } from 'node:path';
import { test } from 'node:test';
import { resolveContractsPath } from '../src/lib/paths.mjs';

function fixture(t) {
  const workspace = mkdtempSync(resolve(tmpdir(), 'roster-paths-'));
  t.after(() => rmSync(workspace, { recursive: true, force: true }));
  const repoRoot = resolve(workspace, 'roster');
  const cwd = resolve(workspace, 'another-project');
  mkdirSync(repoRoot);
  mkdirSync(cwd);

  return {
    options: { repoRoot, cwd, env: {} },
    vendor: resolve(repoRoot, 'vendor', 'github-agent-contracts'),
    configured: resolve(workspace, 'agent contracts'),
    sibling: resolve(workspace, 'github-agent-contracts'),
  };
}

function addPublisher(contractsPath) {
  const scriptsPath = resolve(contractsPath, 'scripts');
  mkdirSync(scriptsPath, { recursive: true });
  writeFileSync(resolve(scriptsPath, 'agent-pr.mjs'), 'export {};\n');
}

test('prefers the roster submodule over environment and sibling clones, regardless of cwd', (t) => {
  const { options, vendor, configured, sibling } = fixture(t);
  for (const contractsPath of [vendor, configured, sibling]) {
    addPublisher(contractsPath);
  }

  assert.equal(
    resolveContractsPath({ ...options, env: { GITHUB_AGENT_CONTRACTS: configured } }),
    vendor,
  );
});

test('uses the submodule without an environment override', (t) => {
  const { options, vendor } = fixture(t);
  addPublisher(vendor);

  assert.equal(resolveContractsPath(options), vendor);
});

test('defaults to the sibling clone beside the roster rather than the caller cwd', (t) => {
  const { options, sibling } = fixture(t);
  addPublisher(sibling);
  addPublisher(resolve(options.cwd, 'vendor', 'github-agent-contracts'));

  assert.equal(resolveContractsPath(options), sibling);
});

test('resolves relative overrides against the caller cwd before trying the sibling', (t) => {
  const { options, configured, sibling } = fixture(t);
  addPublisher(configured);
  addPublisher(sibling);

  const configuredPath = relative(options.cwd, configured);
  assert.equal(
    resolveContractsPath({ ...options, env: { GITHUB_AGENT_CONTRACTS: configuredPath } }),
    configured,
  );
});

test('preserves spaces and normalizes absolute overrides', (t) => {
  const { options, configured } = fixture(t);
  addPublisher(configured);
  const configuredPath = `${configured}${sep}nested${sep}..`;

  assert.equal(
    resolveContractsPath({ ...options, env: { GITHUB_AGENT_CONTRACTS: configuredPath } }),
    configured,
  );
});

test('skips an uninitialized submodule and uses the environment clone', (t) => {
  const { options, vendor, configured } = fixture(t);
  mkdirSync(vendor, { recursive: true });
  addPublisher(configured);

  assert.equal(
    resolveContractsPath({ ...options, env: { GITHUB_AGENT_CONTRACTS: configured } }),
    configured,
  );
});

test('skips an incomplete environment clone and uses the sibling', (t) => {
  const { options, configured, sibling } = fixture(t);
  mkdirSync(configured);
  addPublisher(sibling);

  assert.equal(
    resolveContractsPath({ ...options, env: { GITHUB_AGENT_CONTRACTS: configured } }),
    sibling,
  );
});

test('skips a nonexistent environment clone and uses the sibling', (t) => {
  const { options, configured, sibling } = fixture(t);
  addPublisher(sibling);

  assert.equal(
    resolveContractsPath({ ...options, env: { GITHUB_AGENT_CONTRACTS: configured } }),
    sibling,
  );
});

test('does not accept a directory named scripts/agent-pr.mjs as the publisher', (t) => {
  const { options, vendor } = fixture(t);
  mkdirSync(resolve(vendor, 'scripts', 'agent-pr.mjs'), { recursive: true });

  assert.throws(() => resolveContractsPath(options), /no scripts\/agent-pr\.mjs file found/);
});

test('fails with the searched locations and initialization command when no publisher exists', (t) => {
  const { options, vendor, configured, sibling } = fixture(t);

  assert.throws(
    () => resolveContractsPath({ ...options, env: { GITHUB_AGENT_CONTRACTS: configured } }),
    (error) => {
      assert.match(error.message, /github-agent-contracts is required/);
      assert.match(error.message, /scripts\/agent-pr\.mjs/);
      for (const contractsPath of [vendor, configured, sibling]) {
        assert.ok(error.message.includes(contractsPath), error.message);
      }
      assert.match(error.message, /git submodule update --init --recursive/);
      return true;
    },
  );
});

test('rejects blank overrides instead of resolving to cwd', (t) => {
  const { options } = fixture(t);
  for (const configuredPath of ['', '  ', '\t']) {
    assert.throws(
      () => resolveContractsPath({ ...options, env: { GITHUB_AGENT_CONTRACTS: configuredPath } }),
      { name: 'TypeError', message: 'GITHUB_AGENT_CONTRACTS must be a non-empty path' },
    );
  }
});

test('rejects non-string overrides', (t) => {
  const { options } = fixture(t);
  for (const configuredPath of [42, null, false, {}]) {
    assert.throws(
      () => resolveContractsPath({ ...options, env: { GITHUB_AGENT_CONTRACTS: configuredPath } }),
      { name: 'TypeError', message: 'GITHUB_AGENT_CONTRACTS must be a non-empty path' },
    );
  }
});
