import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { formatCapabilities, loadCapabilities, parseCapabilities, validateCapabilities } from '../src/lib/capabilities.mjs';

const example = readFileSync(new URL('../examples/capabilities.yml', import.meta.url), 'utf8');
const record = { profile_id: 'default', task_class: 'fix', suggested_difficulty: 2,
  context_max: 8192, concurrency: 1, notes: 'Starting guess, not a benchmark.' };

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'roster-capabilities-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const installationRoot = join(root, 'install');
  const cwd = join(root, 'project');
  mkdirSync(join(installationRoot, 'examples'), { recursive: true });
  mkdirSync(join(cwd, '.roster'), { recursive: true });
  copyFileSync(new URL('../examples/capabilities.yml', import.meta.url),
    join(installationRoot, 'examples', 'capabilities.yml'));
  return { cwd, installationRoot, file: join(cwd, '.roster', 'capabilities.yml') };
}

test('tracked hypothetical priors validate and round-trip without registering a model', () => {
  const catalog = parseCapabilities(example);
  assert.equal(catalog.capabilities.length, 6);
  assert.deepEqual(parseCapabilities(formatCapabilities(catalog)), catalog);
  assert.equal(Object.isFrozen(catalog.capabilities[0]), true);
  assert.ok(catalog.capabilities.every(({ notes }) => /guess|illustration/.test(notes)));
});

test('private overlay merges fields by selector and class while preserving other priors', async (t) => {
  const options = fixture(t);
  const baseline = await loadCapabilities(options);
  const overlay = { capabilities: [
    { profile_id: 'default', task_class: 'fix', suggested_difficulty: 4 },
    { model_id: 'owner/local-model', task_class: 'docs', suggested_difficulty: 3,
      context_max: 32768, concurrency: 2, notes: 'Local starting guess.' },
  ] };
  writeFileSync(options.file, formatCapabilities(overlay, { partial: true }));
  const loaded = await loadCapabilities(options);
  const updated = loaded.capabilities.find((row) => row.profile_id === 'default' && row.task_class === 'fix');
  assert.equal(updated.suggested_difficulty, 4);
  assert.equal(updated.context_max, 8192);
  assert.equal(loaded.capabilities.length, baseline.capabilities.length + 1);
  assert.equal(loaded.capabilities.find(({ model_id }) => model_id === 'owner/local-model').concurrency, 2);
});

test('invalid difficulties, capacities, duplicates, selectors and IP-bearing notes are refused', () => {
  for (const changes of [
    { suggested_difficulty: 0 }, { suggested_difficulty: 6 }, { context_max: 0 },
    { concurrency: 0 }, { model_id: 'owner/model' }, { task_class: 'deploy' },
    { notes: 'Private host 192.0.2.10' }, { notes: 'IPv6 host [2001:db8::1]' },
    { notes: 'x'.repeat(241) }, { extra: 'unexpected' },
  ]) {
    assert.throws(() => validateCapabilities({ capabilities: [{ ...record, ...changes }] }));
  }
  assert.throws(() => validateCapabilities({ capabilities: [record, record] }), /unique/);
  assert.throws(() => parseCapabilities('capabilities: invalid\n'), /Catalog/);
});

test('missing overlays use the example, but malformed or incomplete new records fail explicitly', async (t) => {
  const options = fixture(t);
  assert.deepEqual(await loadCapabilities(options), parseCapabilities(example));
  writeFileSync(options.file, 'capabilities:\n  - model_id: unknown\n    task_class: fix\n');
  await assert.rejects(loadCapabilities(options), /set model/);
  writeFileSync(options.file, 'capabilities:\n  - model_id: owner/new\n    task_class: fix\n');
  await assert.rejects(loadCapabilities(options), /documented fields/);
  writeFileSync(options.file, Buffer.from([0xff]));
  await assert.rejects(loadCapabilities(options), /UTF-8/);
});
