import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, promises as fs, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { writePrivateDocuments } from '../src/lib/private-files.mjs';

function fixture(t) {
  const repoRoot = mkdtempSync(join(tmpdir(), 'roster-private-'));
  t.after(() => rmSync(repoRoot, { recursive: true, force: true }));
  mkdirSync(join(repoRoot, '.roster'));
  return repoRoot;
}

test('paired private writes roll back a replaced fleet if the config save fails', async (t) => {
  const repoRoot = fixture(t);
  writeFileSync(join(repoRoot, '.roster', 'fleet.yml'), 'old fleet\n');
  writeFileSync(join(repoRoot, '.roster', 'config.yml'), 'old config\n');
  await assert.rejects(writePrivateDocuments([
    { name: 'fleet.yml', source: 'new fleet\n', expectedSource: 'old fleet\n' },
    { name: 'config.yml', source: 'new config\n', expectedSource: 'old config\n' },
  ], {
    repoRoot, fileSystem: { ...fs, async rename(source, destination) {
      if (destination.endsWith('config.yml')) throw new Error('Config save denied');
      await fs.rename(source, destination);
    } },
  }), /Config save denied/);
  assert.equal(readFileSync(join(repoRoot, '.roster', 'fleet.yml'), 'utf8'), 'old fleet\n');
  assert.equal(readFileSync(join(repoRoot, '.roster', 'config.yml'), 'utf8'), 'old config\n');
  const files = readdirSync(join(repoRoot, '.roster'));
  assert.deepEqual(files.filter((name) => !name.endsWith('.bak')).sort(), ['config.yml', 'fleet.yml']);
  assert.equal(files.filter((name) => name.endsWith('.bak')).length, 2);
});

test('successful replacement retains exact previous bytes; identical saves create no backup', async (t) => {
  const repoRoot = fixture(t);
  writeFileSync(join(repoRoot, '.roster', 'fleet.yml'), 'old fleet\r\n');
  await writePrivateDocuments([{ name: 'fleet.yml', source: 'new fleet\n',
    expectedSource: 'old fleet\r\n' }], { repoRoot });
  const files = readdirSync(join(repoRoot, '.roster'));
  const backup = files.find((name) => name.endsWith('.bak'));
  assert.ok(backup);
  assert.equal(readFileSync(join(repoRoot, '.roster', backup), 'utf8'), 'old fleet\r\n');
  await writePrivateDocuments([{ name: 'fleet.yml', source: 'new fleet\n',
    expectedSource: 'new fleet\n' }], { repoRoot });
  assert.deepEqual(readdirSync(join(repoRoot, '.roster')), files);
});

test('stale private settings are refused before either document is written', async (t) => {
  const repoRoot = fixture(t);
  writeFileSync(join(repoRoot, '.roster', 'config.yml'), 'externally updated\n');
  await assert.rejects(writePrivateDocuments([
    { name: 'fleet.yml', source: 'new fleet\n', expectedSource: null },
    { name: 'config.yml', source: 'new config\n', expectedSource: 'old config\n' },
  ], { repoRoot }), /changed during setup/);
  assert.deepEqual(readdirSync(join(repoRoot, '.roster')), ['config.yml']);
  assert.equal(readFileSync(join(repoRoot, '.roster', 'config.yml'), 'utf8'), 'externally updated\n');
});

test('backup failure refuses replacement and removes temporary files', async (t) => {
  const repoRoot = fixture(t);
  writeFileSync(join(repoRoot, '.roster', 'config.yml'), 'old config\n');
  await assert.rejects(writePrivateDocuments([
    { name: 'config.yml', source: 'new config\n', expectedSource: 'old config\n' },
  ], { repoRoot, fileSystem: { ...fs, async writeFile(file, ...args) {
    if (file.endsWith('.bak')) throw new Error('Backup denied');
    return fs.writeFile(file, ...args);
  } } }), /Backup denied/);
  assert.deepEqual(readdirSync(join(repoRoot, '.roster')), ['config.yml']);
  assert.equal(readFileSync(join(repoRoot, '.roster', 'config.yml'), 'utf8'), 'old config\n');
});
