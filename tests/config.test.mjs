import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { ConfigError, loadConfig, parseConfig } from '../src/lib/config.mjs';

const example = readFileSync(new URL('../roster.config.example.yml', import.meta.url), 'utf8');

function fixture(context) {
  const root = mkdtempSync(join(tmpdir(), 'roster-config-'));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(join(root, 'roster.config.example.yml'), example);
  return root;
}

test('loads the tracked example when private config is absent', (context) => {
  const repoRoot = fixture(context);
  const config = loadConfig({ repoRoot });
  assert.deepEqual(config, {
    schema: 1,
    llm: {
      base_url: '', model: '', api_key_env: 'ROSTER_API_KEY', effort: 'm', context_max: 0,
    },
    seat: {
      id: 'coder', principal: 'coder', turn_budget: 8,
      tools: ['read_file', 'write_file', 'list_dir', 'run_test'],
    },
    paths: {
      memory: join('.roster', 'memory', 'coder.jsonl'),
      skills: 'skills', asks: join('.roster', 'asks'), worktrees: '.worktrees',
    },
  });
  assert.equal(Object.isFrozen(config.seat.tools), true);
  assert.equal(Object.isFrozen(config.llm), true);
});

test('private config overrides the example without resolving or logging the API key', (context) => {
  const repoRoot = fixture(context);
  mkdirSync(join(repoRoot, '.roster'));
  writeFileSync(join(repoRoot, '.roster', 'config.yml'),
    example.replace('base_url: ""', 'base_url: "http://localhost:1234/v1"')
      .replace('model: ""', 'model: local-model')
      .replace('turn_budget: 8', 'turn_budget: 3'));
  const original = process.env.ROSTER_API_KEY;
  process.env.ROSTER_API_KEY = 'do-not-print-this-secret';
  try {
    const config = loadConfig({ repoRoot });
    assert.equal(config.llm.base_url, 'http://localhost:1234/v1');
    assert.equal(config.seat.turn_budget, 3);
    assert.ok(!JSON.stringify(config).includes(process.env.ROSTER_API_KEY));
  } finally {
    if (original === undefined) delete process.env.ROSTER_API_KEY;
    else process.env.ROSTER_API_KEY = original;
  }
});

test('rejects invalid schema, fields, roles, paths, tool names, and endpoint settings', () => {
  for (const [name, source] of [
    ['missing', example.replace(/  effort: m[^\r\n]*\r?\n/, '')],
    ['duplicate', example.replace('schema: 1', 'schema: 1\nschema: 1')],
    ['schema', example.replace('schema: 1', 'schema: 2')],
    ['extra', `${example}merge: true\n`],
    ['principal', example.replace('principal: coder', 'principal: merger')],
    ['tools', example.replace('run_test]', 'git_push]')],
    ['budget', example.replace('turn_budget: 8', 'turn_budget: 0')],
    ['effort', example.replace('effort: m ', 'effort: max ')],
    ['traversal', example.replace('skills: skills', 'skills: ../outside')],
    ['absolute', example.replace('skills: skills', 'skills: C:\\outside')],
    ['no model', example.replace('base_url: ""', 'base_url: http://localhost:1234/v1')],
    ['credentials', example.replace('base_url: ""', 'base_url: https://name:secret@api.example/v1')
      .replace('model: ""', 'model: test-model')],
  ]) {
    assert.throws(() => parseConfig(source), ConfigError, name);
  }
  assert.throws(() => parseConfig('x'.repeat(65_537)), /at most 64 KiB/);
});

test('only a missing private config triggers the example fallback', (context) => {
  const repoRoot = fixture(context);
  const privateFile = join(repoRoot, '.roster', 'config.yml');
  mkdirSync(join(repoRoot, '.roster'));
  writeFileSync(privateFile, 'bad: input\n');
  assert.throws(() => loadConfig({ repoRoot }), ConfigError);
  rmSync(privateFile);
  writeFileSync(privateFile, Buffer.from([0xff]));
  assert.throws(() => loadConfig({ repoRoot }), /UTF-8/);
  rmSync(privateFile);
  try {
    symlinkSync(fileURLToPath(new URL('../roster.config.example.yml', import.meta.url)), privateFile);
  } catch (error) {
    if (['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) {
      context.skip('Creating symlinks is unavailable on this system.');
      return;
    }
    throw error;
  }
  assert.throws(() => loadConfig({ repoRoot }), /non-symlink/);
});
