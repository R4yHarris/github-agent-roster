import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { formatFleet, getFleetProfile, loadFleet, normalizeFleetBaseUrl,
  parseFleet, validateFleet, validateFleetProfile, withFleetProfile } from '../src/lib/fleet.mjs';

const example = readFileSync(new URL('../examples/fleet.yml', import.meta.url), 'utf8');
const profile = {
  id: 'test-coder', base_url: 'http://localhost:8000/v1', model: 'owner/model', provider: 'vllm',
  context_max: 32768, concurrency: 2, hardware: 'test-gpu', task_class: ['fix', 'docs'], notes: '',
};

test('fictional catalog parses and round-trips without becoming a configured fleet', () => {
  const fleet = parseFleet(example);
  assert.equal(fleet.profiles.length, 3);
  assert.equal(fleet.profiles.find(({ id }) => id === 'example-large-context').base_url, 'https://cluster.example.invalid/v1');
  const spark = fleet.profiles.find(({ id }) => id === 'spark-4');
  assert.equal(spark.hardware, 'spark-4');
  assert.equal(spark.context_max, 1048576);
  assert.match(spark.notes, /512GB class/);
  assert.deepEqual(parseFleet(formatFleet(fleet)), fleet);
  assert.ok(fleet.profiles.every(({ base_url }) => new URL(base_url).hostname.endsWith('.example.invalid')));
  assert.equal(Object.isFrozen(fleet.profiles), true);
  assert.equal(Object.isFrozen(fleet.profiles[0].task_class), true);
  assert.deepEqual(parseFleet('profiles: []\n'), { profiles: [] });
});

test('normalizes HTTP base URLs and refuses credentials and full request URLs', () => {
  assert.equal(normalizeFleetBaseUrl('http://localhost:8000'), 'http://localhost:8000/v1');
  assert.equal(normalizeFleetBaseUrl('https://example.invalid/api/v1/'), 'https://example.invalid/api/v1');
  for (const value of ['file:///private', 'http://user:secret@example.invalid',
    'http://example.invalid?token=secret', 'http://example.invalid#fragment',
    'http://localhost/v1/models', 'http://localhost/v1/chat/completions', 'bad', '']) {
    assert.throws(() => normalizeFleetBaseUrl(value), /URL|base_url|nonempty/);
  }
});

test('unique IDs, real models, positive capacity, and optional task hints are validated', () => {
  assert.deepEqual(validateFleetProfile(profile), profile);
  const withoutHints = { ...profile };
  delete withoutHints.task_class;
  assert.equal(validateFleetProfile(withoutHints).task_class, undefined);
  for (const changed of [
    { id: '../escape' }, { model: 'unknown' }, { model: 'builtin-stub' }, { provider: 'unknown' },
    { context_max: 0 }, { context_max: -1 }, { context_max: '32768' },
    { context_max: Number.MAX_SAFE_INTEGER + 1 }, { concurrency: 0 }, { concurrency: 1.5 },
    { task_class: ['fix', 'fix'] }, { task_class: ['deploy'] }, { task_class: [] },
    { hardware: 'bad\nhardware' }, { notes: 'bad\0note' }, { unrecognized: true },
  ]) {
    assert.throws(() => validateFleetProfile({ ...profile, ...changed }));
  }
  assert.throws(() => validateFleet({ profiles: [profile, profile] }), /unique/);
  assert.equal(getFleetProfile({ profiles: [profile] }, 'test-coder'), profile);
  assert.throws(() => getFleetProfile({ profiles: [profile] }, 'missing'), /not found/);
  assert.equal(validateFleetProfile({ ...profile, id: 'default', context_max: 0 }).context_max, 0);
  assert.throws(() => validateFleetProfile({ ...profile, id: 'default', context_max: -1 }), /context_max/);
  assert.equal(validateFleetProfile({ ...profile, request_timeout_ms: 1_200_000 }).request_timeout_ms, 1_200_000);
  assert.throws(() => validateFleetProfile({ ...profile, request_timeout_ms: 0 }), /request_timeout_ms/);
  const selected = withFleetProfile({
    llm: { request_timeout_ms: 120_000 },
    profiles: { 'vllm-local': { api_key_env: 'ROSTER_API_KEY', api_key_optional: true } },
  }, { ...profile, request_timeout_ms: 1_200_000 });
  assert.equal(selected.llm.request_timeout_ms, 1_200_000);
});

test('unsupported YAML and duplicate catalog keys fail instead of being silently ignored', () => {
  const source = formatFleet({ profiles: [profile] });
  for (const invalid of [
    '', source + 'profiles: []\n', source.replace('    concurrency: 2', '    concurrency: 2\n    concurrency: 3'),
    source.replace('  - id:', '\t- id:'), source.replace('    provider:', '   provider:'),
    source.replace('    notes: ""', '    extra: true'), source.replace('32768', '9007199254740993'),
    source.replace('    notes: ""', '    notes: &alias note'), '# bad\0comment\n' + source,
  ]) {
    assert.throws(() => parseFleet(invalid), /Catalog|Fleet/);
  }
  assert.throws(() => parseFleet('x'.repeat(65_537)), /64 KiB/);
  assert.deepEqual(parseFleet(source.replace(/\n/g, '\r\n')), validateFleet({ profiles: [profile] }));
});

test('private fleet resolves from the worktree and absent catalogs do not load examples', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'roster-fleet-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  assert.deepEqual(await loadFleet({ cwd: root }), { profiles: [] });
  const nested = join(root, 'nested');
  mkdirSync(nested);
  mkdirSync(join(root, '.roster'));
  copyFileSync(new URL('../.gitignore', import.meta.url), join(root, '.gitignore'));
  execFileSync('git', ['init', '--quiet'], { cwd: root, stdio: 'pipe' });
  const file = join(root, '.roster', 'fleet.yml');
  writeFileSync(file, formatFleet({ profiles: [profile] }));
  assert.deepEqual(await loadFleet({ cwd: nested }), validateFleet({ profiles: [profile] }));
  assert.doesNotThrow(() => execFileSync('git', ['check-ignore', '--quiet', '.roster/fleet.yml'], {
    cwd: root, stdio: 'pipe',
  }));
  writeFileSync(file, 'profiles: invalid\n');
  await assert.rejects(loadFleet({ cwd: root }), /Catalog/);
  writeFileSync(file, Buffer.from([0xff]));
  await assert.rejects(loadFleet({ cwd: root }), /UTF-8/);
  rmSync(file);
  mkdirSync(file);
  await assert.rejects(loadFleet({ cwd: root }), /regular/);
  rmSync(file, { recursive: true });
  const target = join(root, 'source.yml');
  writeFileSync(target, example);
  try {
    symlinkSync(target, file);
  } catch (error) {
    if (!['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) throw error;
    t.skip('Creating symlinks is unavailable on this system.');
    return;
  }
  await assert.rejects(loadFleet({ cwd: root }), /symlinks/);
});
