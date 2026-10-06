import test from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import {
  openRepoState, resolveRepoState, atomicWriteFile, readRepoStateFile, writeRepoStateFile,
  readMarker, SCHEMA_MARKER, CURRENT_SCHEMA_VERSION, RepoStateError,
} from '../src/lib/repo-state.mjs';
import { identityHash } from '../src/lib/repo-identity.mjs';

async function withTempDir(fn) {
  const dir = await mkdtemp(join(tmpdir(), 'roster-state-'));
  try {
    return await fn(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

test('first use creates the state dir with a schema/version marker', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'repos', 'test-repo');
    const { marker, state } = await openRepoState(stateRoot, { identity: 'sha256-' + 'a'.repeat(64) });
    assert.equal(marker.version, CURRENT_SCHEMA_VERSION);
    assert.deepEqual(state, {});
    const onDisk = JSON.parse(await fs.readFile(join(stateRoot, SCHEMA_MARKER), 'utf8'));
    assert.equal(onDisk.version, CURRENT_SCHEMA_VERSION);
    const identity = (await fs.readFile(join(stateRoot, 'identity'), 'utf8')).trim();
    assert.equal(identity, 'sha256-' + 'a'.repeat(64));
  });
});

test('fresh clone with no state dir yields empty state without errors', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'fresh', 'clone');
    const { state } = await openRepoState(stateRoot);
    assert.deepEqual(state, {});
    const marker = await readMarker(stateRoot);
    assert.equal(marker.version, CURRENT_SCHEMA_VERSION);
  });
});

test('resolveRepoState returns the resolver root and writes the marker on first use', async () => {
  await withTempDir(async (root) => {
    const repoRoot = join(root, 'checkout');
    const machineRoot = join(root, 'machine');
    await fs.mkdir(join(repoRoot, '.git'), { recursive: true });
    await fs.mkdir(machineRoot, { recursive: true });
    const identity = 'sha256-' + 'c'.repeat(64);
    const opened = await resolveRepoState({
      repoRoot, machineRoot, deriveIdentity: async () => identity,
    });
    // The #198 resolver scopes repo state under <machineRoot>/repos/<repoId>.
    assert.match(opened.stateRoot, /repos/);
    assert.deepEqual(opened.state, {});
    assert.equal(opened.marker.version, CURRENT_SCHEMA_VERSION);
    const onDisk = JSON.parse(await fs.readFile(join(opened.stateRoot, SCHEMA_MARKER), 'utf8'));
    assert.equal(onDisk.version, CURRENT_SCHEMA_VERSION);
    assert.equal((await fs.readFile(join(opened.stateRoot, 'identity'), 'utf8')).trim(), identity);
    // Reopening the same resolved root is stable and does not throw.
    const again = await resolveRepoState({
      repoRoot, machineRoot, deriveIdentity: async () => identity,
    });
    assert.equal(again.stateRoot, opened.stateRoot);
    // A StateHandle from the #198 resolver is reused verbatim.
    const { resolveStateRoot } = await import('../src/lib/paths.mjs');
    const handle = await resolveStateRoot({ repoRoot, machineRoot });
    const fromHandle = await resolveRepoState({
      stateRoot: handle, deriveIdentity: async () => identity,
    });
    assert.equal(fromHandle.stateRoot, handle.root);
  });
});

test('schema marker version is the current version and versioning fails closed', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'state');
    await openRepoState(stateRoot);
    const marker = await readMarker(stateRoot);
    assert.equal(marker.version, CURRENT_SCHEMA_VERSION);
    // A marker at any other version is refused, naming the file and versions.
    await fs.writeFile(join(stateRoot, SCHEMA_MARKER), JSON.stringify({ version: '2' }), 'utf8');
    await assert.rejects(
      () => openRepoState(stateRoot),
      (error) => error.code === 'E_MARKER_VERSION' &&
        error.message.includes('"2"') &&
        error.message.includes(String(CURRENT_SCHEMA_VERSION)) &&
        error.path === join(stateRoot, SCHEMA_MARKER),
    );
  });
});

test('marker version mismatch fails closed with expected vs found and recovery hint', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'state');
    await fs.mkdir(stateRoot, { recursive: true });
    await fs.writeFile(join(stateRoot, SCHEMA_MARKER), JSON.stringify({ version: 999 }), 'utf8');
    await assert.rejects(
      () => openRepoState(stateRoot),
      (error) => error.code === 'E_MARKER_VERSION' && /999/.test(error.message) &&
        String(CURRENT_SCHEMA_VERSION).length > 0,
    );
  });
});

test('missing marker in an existing state dir fails closed, no reinitialization', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'state');
    await fs.mkdir(stateRoot, { recursive: true });
    await fs.writeFile(join(stateRoot, 'unrelated.txt'), 'keep', 'utf8');
    await assert.rejects(
      () => openRepoState(stateRoot),
      (error) => error.code === 'E_MARKER_MISSING' && /schema marker/.test(error.message) &&
        /deleting the state directory/.test(error.message),
    );
    // Unrelated files are untouched; no marker was silently written.
    assert.equal(await fs.readFile(join(stateRoot, 'unrelated.txt'), 'utf8'), 'keep');
    const entries = await fs.readdir(stateRoot);
    assert.equal(entries.includes(SCHEMA_MARKER), false);
  });
});

test('interrupted marker write (temp file, no marker) fails closed, not empty', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'state');
    await fs.mkdir(stateRoot, { recursive: true });
    await fs.writeFile(join(stateRoot, `${SCHEMA_MARKER}.deadbeef.tmp`), 'partial', 'utf8');
    await assert.rejects(
      () => openRepoState(stateRoot),
      (error) => error.code === 'E_INTERRUPTED_WRITE' && /interrupted atomic write/.test(error.message),
    );
    // No marker was fabricated and the leftover temp file is still there.
    const entries = await fs.readdir(stateRoot);
    assert.equal(entries.includes(SCHEMA_MARKER), false);
  });
});

test('corrupt marker fails closed with an actionable error', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'state');
    await fs.mkdir(stateRoot, { recursive: true });
    await fs.writeFile(join(stateRoot, SCHEMA_MARKER), '{not json', 'utf8');
    await assert.rejects(
      () => openRepoState(stateRoot),
      (error) => error.code === 'E_MARKER_CORRUPT' && /unreadable or corrupt/.test(error.message),
    );
  });
});

test('identity mismatch refuses to open; matching identity re-opens cleanly', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'state');
    await openRepoState(stateRoot, { identity: 'sha256-' + 'a'.repeat(64) });
    await assert.rejects(
      () => openRepoState(stateRoot, { identity: 'sha256-' + 'b'.repeat(64) }),
      (error) => error.code === 'E_IDENTITY_MISMATCH' && /does not match/.test(error.message),
    );
    // Refusing leaves the recorded identity intact.
    assert.equal((await fs.readFile(join(stateRoot, 'identity'), 'utf8')).trim(),
      'sha256-' + 'a'.repeat(64));
    const { marker } = await openRepoState(stateRoot, { identity: 'sha256-' + 'a'.repeat(64) });
    assert.equal(marker.version, CURRENT_SCHEMA_VERSION);
  });
});

test('identity mismatch with the documented reinitialize policy records the new identity', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'state');
    const first = 'sha256-' + 'a'.repeat(64);
    const second = 'sha256-' + 'b'.repeat(64);
    await openRepoState(stateRoot, { identity: first });
    const opened = await openRepoState(stateRoot, {
      identity: second, onIdentityMismatch: 'reinitialize',
    });
    assert.equal(opened.identity.reinitialized, true);
    assert.equal(opened.identity.from, first);
    assert.equal(opened.identity.to, second);
    assert.equal((await fs.readFile(join(stateRoot, 'identity'), 'utf8')).trim(), second);
    // Subsequent opens accept the re-initialized identity.
    const { marker } = await openRepoState(stateRoot, { identity: second });
    assert.equal(marker.version, CURRENT_SCHEMA_VERSION);
    // An unknown policy is rejected rather than guessed.
    await assert.rejects(
      () => openRepoState(stateRoot, { identity: first, onIdentityMismatch: 'squash' }),
      (error) => error.code === 'E_IDENTITY_POLICY',
    );
  });
});

test('identity re-initialize path: no recorded identity means initialize', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'state');
    // Initialize without identity, then open with one: treated as initialize.
    await openRepoState(stateRoot);
    const { marker } = await openRepoState(stateRoot);
    assert.equal(marker.version, CURRENT_SCHEMA_VERSION);
    const identity = 'sha256-' + 'd'.repeat(64);
    await openRepoState(stateRoot, { identity });
    assert.equal((await fs.readFile(join(stateRoot, 'identity'), 'utf8')).trim(), identity);
  });
});

test('identity is hashed: a sentinel remote never appears in recorded state', async () => {
  await withTempDir(async (root) => {
    const stateRoot = join(root, 'state');
    const sentinel = 'test-only-private-api-key';
    const identity = identityHash({ gitCommonDir: '/repo/.git', remoteUrl: sentinel });
    await openRepoState(stateRoot, { identity });
    const recorded = await fs.readFile(join(stateRoot, 'identity'), 'utf8');
    assert.equal(recorded.includes(sentinel), false);
    assert.match(recorded.trim(), /^sha256-[0-9a-f]{64}$/);
  });
});

test('atomic write leaves previous contents readable on temp-write failure', async () => {
  await withTempDir(async (root) => {
    const file = join(root, 'state.json');
    await writeRepoStateFile(file, { turn: 1 });
    await assert.rejects(
      () => atomicWriteFile(join(root, 'missing-dir', 'state.json'), '{}'),
    );
    assert.equal(await fs.readFile(file, 'utf8'), JSON.stringify({ turn: 1 }, null, 2) + '\n');
    const entries = await fs.readdir(root);
    assert.equal(entries.some((entry) => entry.endsWith('.tmp')), false);
  });
});

test('injected rename failure leaves previous contents readable and is typed', async () => {
  await withTempDir(async (root) => {
    const file = join(root, 'state.json');
    await writeRepoStateFile(file, { turn: 1 });
    const previous = await fs.readFile(file, 'utf8');
    try {
      await atomicWriteFile(file, '{"turn":2}\n', {
        rename: async () => {
          const error = new Error('injected rename failure');
          error.code = 'EIO';
          throw error;
        },
      });
      assert.fail('rename failure should have been thrown');
    } catch (error) {
      assert.equal(error.code, 'E_ATOMIC_RENAME');
      assert.equal(error.path, file);
      assert.match(error.message, /previous contents/);
    }
    // The previous file is readable; the leftover temp file is the detector.
    assert.equal(await fs.readFile(file, 'utf8'), previous);
    const leftovers = (await fs.readdir(root)).filter((entry) => entry.endsWith('.tmp'));
    assert.equal(leftovers.length, 1);
    assert.equal(await readRepoStateFile(file), previous);
    await rm(join(root, leftovers[0]), { force: true });
  });
});

test('interrupted write (temp file, missing target) fails closed on next read', async () => {
  await withTempDir(async (root) => {
    const file = join(root, 'state.json');
    const interrupted = `${file}.deadbeef.tmp`;
    await fs.writeFile(interrupted, 'partial', 'utf8');
    await assert.rejects(
      () => readRepoStateFile(file),
      (error) => error.code === 'E_INTERRUPTED_WRITE' && /interrupted atomic write/.test(error.message),
    );
    await rm(interrupted, { force: true });
    // Without a leftover temp file, a plain missing file is a normal ENOENT.
    await assert.rejects(() => readRepoStateFile(file), (error) => error.code === 'ENOENT');
  });
});

test('atomic write then read round-trips', async () => {
  await withTempDir(async (root) => {
    const file = join(root, 'state.json');
    await writeRepoStateFile(file, { commits: [1, 2, 3] });
    assert.deepEqual(JSON.parse(await readRepoStateFile(file)), { commits: [1, 2, 3] });
  });
});

test('openRepoState rejects an unusable state root', async () => {
  await assert.rejects(() => openRepoState(''), RepoStateError);
  await assert.rejects(() => openRepoState('   '), RepoStateError);
  await assert.rejects(() => openRepoState(undefined), RepoStateError);
});