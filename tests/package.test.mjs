import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const manifest = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));

test('npx exposes the Node 20 ESM roster binary without runtime dependencies', () => {
  assert.equal(manifest.type, 'module');
  assert.deepEqual(manifest.bin, { roster: 'src/cli.mjs' });
  assert.equal(manifest.engines.node, '>=20');
  assert.equal(manifest.dependencies, undefined);
});

test('npm files include the pinned publisher but exclude test directories', () => {
  for (const path of ['src/', 'templates/', 'skills/', 'roster.config.example.yml',
    'vendor/github-agent-contracts/scripts/']) {
    assert.ok(manifest.files.includes(path), `missing ${path}`);
  }
  assert.ok(manifest.files.every((path) =>
    path !== 'tests/' && path !== 'vendor/' && path !== 'vendor/github-agent-contracts/tests/'));
});
