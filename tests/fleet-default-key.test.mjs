import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { formatConfig, loadConfig, parseConfig } from '../src/lib/config.mjs';
import { formatFleet, withFleetProfile } from '../src/lib/fleet.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');
const cli = fileURLToPath(new URL('../src/cli.mjs', import.meta.url));
const first = { id: 'first', base_url: 'https://first.example.invalid/v1', model: 'owner/first',
  provider: 'vllm', context_max: 32768, concurrency: 2, hardware: 'fixture-gpu', notes: '', api_key_env: 'CUSTOM_KEY' };
const second = { ...first, id: 'second', base_url: 'https://second.example.invalid/v1',
  model: 'owner/second', context_max: 65536, api_key_env: 'ALT_CUSTOM_KEY' };

function assertSelection(config, profile) {
  for (const name of ['api_key_env', 'base_url', 'model', 'context_max', 'provider']) {
    assert.equal(config.llm[name], profile[name], name);
  }
}

test('a custom fleet selection round-trips without adopting a named preset key', () => {
  let config = parseConfig(example);
  for (const profile of [first, second, first]) {
    const selected = withFleetProfile(config, profile);
    assert.equal(selected.llm.fleet_profile, profile.id);
    assert.equal(selected.llm.concurrency, profile.concurrency);
    assert.equal(selected.llm.hardware, profile.hardware);
    config = parseConfig(formatConfig(selected));
    assertSelection(config, profile);
    assert.equal(config.llm.profile, '');
  }
});

test('fresh CLI invocations preserve two fleet defaults and do not change policy or fetch secrets', (t) => {
  const cwd = mkdtempSync(join(process.cwd(), '.fleet-key-reload-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  execFileSync('git', ['init', '--quiet'], { cwd });
  mkdirSync(join(cwd, '.roster'));
  writeFileSync(join(cwd, '.roster', 'fleet.yml'), formatFleet({ profiles: [first, second] }));
  writeFileSync(join(cwd, 'roster.config.example.yml'), example);
  writeFileSync(join(cwd, '.roster', 'config.yml'), example.replace('enabled: true', 'enabled: false'));
  const env = { ...process.env };
  delete env.NODE_TEST_CONTEXT;
  for (const name of Object.keys(env)) {
    if (/TOKEN|PASSWORD|SECRET|PRIVATE_KEY|API_KEY/i.test(name) || name === 'GITHUB_APP_ID') delete env[name];
  }
  for (const profile of [first, second, first]) {
    const result = spawnSync(process.execPath, [cli, 'fleet', 'default', profile.id], {
      cwd, env, encoding: 'utf8', timeout: 15000,
    });
    assert.equal(result.status, 0, result.stderr);
    const config = loadConfig({ repoRoot: cwd, cwd });
    assertSelection(config, profile);
    assert.equal(config.llm.profile, '');
    assert.equal(config.publish.enabled, false);
    assert.equal(config.reviewer.required, true);
    assert.match(result.stdout, /Default endpoint\/model selected/);
  }
});
