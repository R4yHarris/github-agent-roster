import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { checkAppIdentity, formatAppIdentity } from '../src/onboard/app.mjs';

test('missing App env prints User setx and Unix export instructions without checking an absent path', async () => {
  const result = await checkAppIdentity({
    env: {}, inspect: () => assert.fail('An absent path must not be inspected'),
  });
  assert.deepEqual(result, { envPresent: false, keyFileExists: false });
  const text = formatAppIdentity(result);
  assert.match(text, /setx GITHUB_APP_ID "<app-id>"/);
  assert.match(text, /setx GITHUB_APP_PRIVATE_KEY_PATH "<absolute-path-to-existing-pem>"/);
  assert.match(text, /export GITHUB_APP_ID="<app-id>"/);
  assert.match(text, /restart the shell after setx/);
  assert.match(text, /One GitHub App is used for all worktrees/);
});

test('present App env and PEM metadata produce no secret values or file contents', async () => {
  const cwd = path.resolve('fixture');
  const env = { GITHUB_APP_ID: 'private-app-id', GITHUB_APP_PRIVATE_KEY_PATH: 'private-key-location.pem' };
  const result = await checkAppIdentity({
    cwd, env,
    inspect: async (file) => {
      assert.equal(file, path.resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH));
      return { isFile: () => true };
    },
  });
  const text = formatAppIdentity(result);
  assert.match(text, /^App env present\n/);
  for (const value of Object.values(env)) {
    assert.ok(!text.includes(value));
    assert.ok(!JSON.stringify(result).includes(value));
  }
  assert.equal(result.keyFileExists, true);
});

test('missing or inaccessible key files warn and continue without exposing the path', async () => {
  for (const code of ['ENOENT', 'EACCES']) {
    const env = { GITHUB_APP_ID: 'private-app-id', GITHUB_APP_PRIVATE_KEY_PATH: 'private-key-location.pem' };
    const result = await checkAppIdentity({
      env, inspect: async () => { throw Object.assign(new Error('private-filesystem-details'), { code }); },
    });
    assert.equal(result.envPresent, true);
    assert.equal(result.keyFileExists, false);
    const text = formatAppIdentity(result);
    assert.match(text, /WARNING: App private-key file/);
    assert.doesNotMatch(text, /private-(?:app-id|key-location|filesystem-details)/);
  }
});
