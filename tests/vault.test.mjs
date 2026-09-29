import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createDecipheriv } from 'node:crypto';
import {
  chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync,
  readdirSync, rmSync, statSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { inspect } from 'node:util';
import { createFileVault } from '../src/vault/file.mjs';

const secret = 'test-only-vault-secret \u2603 ';
const entryName = (name) => `${Buffer.from(name).toString('hex')}.json`;

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'roster-vault-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const directory = join(root, 'vault');
  return { root, directory, vault: createFileVault({ directory }) };
}

function safeError(error, pattern) {
  assert.match(error.message, pattern);
  assert.ok(!inspect(error).includes(secret));
  assert.equal(error.cause, undefined);
  return true;
}

test('missing vault reads do not create directories or keys', async (t) => {
  const { directory, vault } = fixture(t);
  assert.equal(await vault.get('MISSING'), undefined);
  assert.deepEqual(await vault.list(), []);
  assert.equal(existsSync(directory), false);
});

test('vault encrypts with AES-256-GCM, persists values, and lists only sorted names', async (t) => {
  const { directory, vault } = fixture(t);
  await vault.set('Z_KEY', secret);
  await vault.set('A_KEY', 'second-test-secret');
  assert.deepEqual(await vault.list(), ['A_KEY', 'Z_KEY']);
  assert.equal(await vault.get('MISSING'), undefined);
  assert.equal(await createFileVault({ directory }).get('Z_KEY'), secret);

  const key = readFileSync(join(directory, '.key'));
  const stored = JSON.parse(readFileSync(join(directory, entryName('Z_KEY')), 'utf8'));
  assert.equal(key.length, 32);
  assert.equal(stored.version, 1);
  assert.equal(Buffer.from(stored.iv, 'base64').length, 12);
  assert.equal(Buffer.from(stored.tag, 'base64').length, 16);
  const decipher = createDecipheriv('aes-256-gcm', key, Buffer.from(stored.iv, 'base64'));
  decipher.setAAD(Buffer.from('Z_KEY'));
  decipher.setAuthTag(Buffer.from(stored.tag, 'base64'));
  assert.equal(Buffer.concat([
    decipher.update(Buffer.from(stored.ciphertext, 'base64')), decipher.final(),
  ]).toString('utf8'), secret);
  key.fill(0);
  for (const file of readdirSync(directory)) {
    const bytes = readFileSync(join(directory, file));
    assert.equal(bytes.includes(Buffer.from(secret)), false);
    assert.equal(bytes.includes(Buffer.from('second-test-secret')), false);
  }
  assert.equal(readdirSync(directory).some((name) => name.startsWith('.tmp-')), false);
});

test('overwrites use fresh nonces and keep the same master key', async (t) => {
  const { directory, vault } = fixture(t);
  await vault.set('TOKEN', secret);
  const key = readFileSync(join(directory, '.key'));
  const before = JSON.parse(readFileSync(join(directory, entryName('TOKEN')), 'utf8'));
  await vault.set('TOKEN', secret);
  const after = JSON.parse(readFileSync(join(directory, entryName('TOKEN')), 'utf8'));
  assert.notEqual(before.iv, after.iv);
  assert.notEqual(before.ciphertext, after.ciphertext);
  assert.deepEqual(readFileSync(join(directory, '.key')), key);
  await vault.set('TOKEN', 'replacement');
  assert.equal(await vault.get('TOKEN'), 'replacement');
  assert.deepEqual(await vault.list(), ['TOKEN']);
  assert.equal(readdirSync(directory).length, 2);
});

test('concurrent first writes share one complete key, including case-distinct names', async (t) => {
  const { directory, vault } = fixture(t);
  const names = ['TOKEN', 'Token', 'SECOND', 'THIRD'];
  await Promise.all(names.map((name) => createFileVault({ directory }).set(name, `value-${name}`)));
  assert.deepEqual(await vault.list(), [...names].sort());
  for (const name of names) assert.equal(await vault.get(name), `value-${name}`);
  assert.equal(readdirSync(directory).length, names.length + 1);
});

test('vault directories and files have owner-only access', async (t) => {
  const { directory, vault } = fixture(t);
  await vault.set('TOKEN', secret);
  if (process.platform !== 'win32') {
    assert.equal(statSync(directory).mode & 0o777, 0o700);
    for (const file of readdirSync(directory)) {
      assert.equal(statSync(join(directory, file)).mode & 0o777, 0o600);
    }
    chmodSync(directory, 0o777);
    chmodSync(join(directory, '.key'), 0o666);
    chmodSync(join(directory, entryName('TOKEN')), 0o666);
    assert.equal(await vault.get('TOKEN'), secret);
    assert.equal(statSync(directory).mode & 0o777, 0o700);
    for (const file of readdirSync(directory)) {
      assert.equal(statSync(join(directory, file)).mode & 0o777, 0o600);
    }
    return;
  }
  const script = [
    "$ErrorActionPreference = 'Stop'",
    '$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value',
    '$paths = @($env:ROSTER_TEST_VAULT) + [System.IO.Directory]::GetFileSystemEntries($env:ROSTER_TEST_VAULT)',
    "foreach ($path in $paths) { if ([System.IO.Directory]::Exists($path)) { $acl = [System.IO.Directory]::GetAccessControl($path) } else { $acl = [System.IO.File]::GetAccessControl($path) }; $rules = $acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]); if ($rules.Count -ne 1 -or $rules[0].IdentityReference.Value -ne $sid -or $rules[0].FileSystemRights -ne 'FullControl' -or $rules[0].AccessControlType -ne 'Allow') { throw 'Vault ACL is not owner-only' } }",
    "Write-Output ('owner-only-access:' + $paths.Count)",
  ].join('; ');
  const result = execFileSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], {
    env: { ...process.env, ROSTER_TEST_VAULT: directory }, encoding: 'utf8', timeout: 10_000,
  });
  assert.equal(result.trim(), 'owner-only-access:3');
});

test('tampered ciphertext and entries moved to another name fail authentication', async (t) => {
  const { directory, vault } = fixture(t);
  await vault.set('TOKEN', secret);
  copyFileSync(join(directory, entryName('TOKEN')), join(directory, entryName('COPIED')));
  await assert.rejects(vault.get('COPIED'), (error) => safeError(error, /authenticated/));
  const path = join(directory, entryName('TOKEN'));
  const entry = JSON.parse(readFileSync(path, 'utf8'));
  const ciphertext = Buffer.from(entry.ciphertext, 'base64');
  ciphertext[0] ^= 1;
  entry.ciphertext = ciphertext.toString('base64');
  writeFileSync(path, JSON.stringify(entry));
  await assert.rejects(vault.get('TOKEN'), (error) => safeError(error, /authenticated/));
});

test('invalid JSON and envelope fields are redacted, not returned as values', async (t) => {
  const { directory, vault } = fixture(t);
  await vault.set('TOKEN', secret);
  const path = join(directory, entryName('TOKEN'));
  for (const content of [
    secret,
    JSON.stringify({ version: 999, ciphertext: secret }),
    JSON.stringify({ version: 1, iv: secret, tag: secret, ciphertext: secret }),
  ]) {
    writeFileSync(path, content);
    await assert.rejects(vault.get('TOKEN'), (error) => safeError(error, /invalid|authenticated/));
  }
});

test('missing or malformed master keys are never silently replaced', async (t) => {
  const { directory, vault } = fixture(t);
  await vault.set('TOKEN', secret);
  const keyPath = join(directory, '.key');
  writeFileSync(keyPath, secret);
  await assert.rejects(vault.get('TOKEN'), (error) => safeError(error, /32 bytes/));
  await assert.rejects(vault.set('OTHER', secret), (error) => safeError(error, /32 bytes/));
  assert.equal(readFileSync(keyPath, 'utf8'), secret);
  rmSync(keyPath);
  await assert.rejects(vault.get('TOKEN'), (error) => safeError(error, /key is missing/));
  await assert.rejects(vault.set('OTHER', secret), (error) => safeError(error, /key is missing/));
  assert.equal(existsSync(keyPath), false);
  assert.deepEqual(await vault.list(), ['TOKEN']);
});

test('invalid secret names and values do not create a vault', async (t) => {
  const { directory, vault } = fixture(t);
  for (const name of ['', '../TOKEN', '..\\TOKEN', 'A/B', 'A:B', 'A\nB', '1TOKEN', 'A'.repeat(65)]) {
    await assert.rejects(vault.set(name, secret), (error) => safeError(error, /Secret names/));
    await assert.rejects(vault.get(name), (error) => safeError(error, /Secret names/));
  }
  for (const value of ['', null, 123]) await assert.rejects(vault.set('TOKEN', value), /non-empty string/);
  assert.equal(existsSync(directory), false);
});

test('vault writes are refused inside repositories and linked worktrees before creating directories', async (t) => {
  const { root } = fixture(t);
  for (const kind of ['repository', 'worktree']) {
    const repository = join(root, kind);
    mkdirSync(repository);
    if (kind === 'repository') mkdirSync(join(repository, '.git'));
    else writeFileSync(join(repository, '.git'), 'gitdir: ../metadata');
    const directory = join(repository, '.roster', 'vault');
    await assert.rejects(createFileVault({ directory }).set('TOKEN', secret),
      (error) => safeError(error, /outside a Git worktree/));
    assert.equal(existsSync(join(repository, '.roster')), false);
  }
});

test('symlinked parents cannot bypass the Git worktree guard', async (t) => {
  const { root } = fixture(t);
  const repository = join(root, 'repository');
  const nested = join(repository, 'nested');
  mkdirSync(nested, { recursive: true });
  mkdirSync(join(repository, '.git'));
  const alias = join(root, 'alias');
  symlinkSync(nested, alias, process.platform === 'win32' ? 'junction' : 'dir');
  const vault = createFileVault({ directory: join(alias, 'vault') });
  await assert.rejects(vault.set('TOKEN', secret), /outside a Git worktree/);
  assert.equal(existsSync(join(nested, 'vault')), false);
});

test('a symlinked vault directory is rejected', async (t) => {
  const { root, directory, vault } = fixture(t);
  const target = join(root, 'target');
  mkdirSync(target);
  symlinkSync(target, directory, process.platform === 'win32' ? 'junction' : 'dir');
  await assert.rejects(vault.set('TOKEN', secret), /not a symbolic link/);
  assert.deepEqual(readdirSync(target), []);
});

test('symlinked entry files are not read or overwritten', async (t) => {
  const { root, directory, vault } = fixture(t);
  await vault.set('TOKEN', secret);
  const target = join(root, 'outside-entry');
  writeFileSync(target, 'unchanged');
  try {
    symlinkSync(target, join(directory, entryName('ALIAS')), 'file');
  } catch (error) {
    if (process.platform !== 'win32' || error.code !== 'EPERM') throw error;
    t.skip('Creating file symlinks requires Windows Developer Mode or the symlink privilege');
    return;
  }
  await assert.rejects(vault.get('ALIAS'), /regular files/);
  await assert.rejects(vault.set('ALIAS', secret), /regular files/);
  await assert.rejects(vault.list(), /invalid entry/);
  assert.equal(readFileSync(target, 'utf8'), 'unchanged');
});

test('filesystem errors are explicit and do not echo path or key material', async (t) => {
  const { root } = fixture(t);
  const parent = join(root, 'not-a-directory');
  writeFileSync(parent, secret);
  const vault = createFileVault({ directory: join(parent, 'vault') });
  await assert.rejects(vault.set('TOKEN', secret), (error) => safeError(error, /Unable to write/));
  assert.equal(readFileSync(parent, 'utf8'), secret);
});
