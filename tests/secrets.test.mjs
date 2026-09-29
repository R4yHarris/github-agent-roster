import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { inspect } from 'node:util';
import { resolveSecret } from '../src/lib/secrets.mjs';
import { createFileVault } from '../src/vault/file.mjs';

test('a non-empty environment value wins without touching the vault', async () => {
  assert.equal(await resolveSecret('TOKEN', {
    env: { TOKEN: 'environment-value' },
    vault: { get: () => assert.fail('The vault must not be read') },
  }), 'environment-value');
});

test('unset and empty environment values fall back to the file vault', async (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'roster-secrets-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const vault = createFileVault({ directory });
  await vault.set('TOKEN', 'vault-value');
  for (const env of [{}, { TOKEN: '' }, Object.create({ TOKEN: 'inherited-value' })]) {
    assert.equal(await resolveSecret('TOKEN', { env, vault }), 'vault-value');
  }
  assert.equal(await resolveSecret('TOKEN', { env: { TOKEN: 'environment-value' }, vault }), 'environment-value');
});

test('a missing secret returns undefined without creating a vault', async (t) => {
  const root = mkdtempSync(join(tmpdir(), 'roster-secrets-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const directory = join(root, 'missing');
  assert.equal(await resolveSecret('MISSING', { env: {}, vault: createFileVault({ directory }) }), undefined);
  assert.equal(existsSync(directory), false);
});

test('vault failures do not become missing secrets or leak material through causes', async () => {
  const secret = 'test-only-secret-that-must-not-leak';
  await assert.rejects(resolveSecret('TOKEN', {
    env: {},
    vault: { get: async () => { throw new Error(secret, { cause: new Error(secret) }); } },
  }), (error) => {
    assert.match(error.message, /Unable to resolve/);
    assert.ok(!inspect(error).includes(secret));
    assert.equal(error.cause, undefined);
    return true;
  });
});

test('invalid names and non-string credentials fail explicitly', async () => {
  await assert.rejects(resolveSecret('../TOKEN', { env: {} }), /Secret names/);
  await assert.rejects(resolveSecret('TOKEN', { env: { TOKEN: 123 } }), /must be a string/);
  for (const value of [null, '', 123]) {
    await assert.rejects(resolveSecret('TOKEN', { env: {}, vault: { get: async () => value } }), /invalid secret/);
  }
});
