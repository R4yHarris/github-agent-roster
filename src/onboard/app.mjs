import { promises as fs } from 'node:fs';
import path from 'node:path';

const present = (value) => typeof value === 'string' && value.trim().length > 0;

export async function checkAppIdentity({
  cwd = process.cwd(), env = process.env, inspect = fs.stat,
} = {}) {
  const envPresent = present(env.GITHUB_APP_ID) && present(env.GITHUB_APP_PRIVATE_KEY_PATH);
  let keyFileExists = false;
  let keyWarning;
  if (present(env.GITHUB_APP_PRIVATE_KEY_PATH)) {
    try {
      const entry = await inspect(path.resolve(cwd, env.GITHUB_APP_PRIVATE_KEY_PATH));
      keyFileExists = entry.isFile();
      if (!keyFileExists) keyWarning = 'App private-key path is not a regular file.';
    } catch (error) {
      if (!(error instanceof Error)) throw error;
      keyWarning = error.code === 'ENOENT'
        ? 'App private-key file is missing.'
        : 'App private-key file could not be checked; inspect local file permissions.';
    }
  }
  return { envPresent, keyFileExists, ...(keyWarning ? { keyWarning } : {}) };
}

export function formatAppIdentity(result) {
  if (typeof result?.envPresent !== 'boolean' || typeof result.keyFileExists !== 'boolean') {
    throw new TypeError('Expected App environment and key-file presence checks');
  }
  let text = result.envPresent ? 'App env present\n' : 'App env missing\n' +
    'Windows User environment (restart the shell after setx):\n' +
    '  setx GITHUB_APP_ID "<app-id>"\n' +
    '  setx GITHUB_APP_PRIVATE_KEY_PATH "<absolute-path-to-existing-pem>"\n' +
    'Unix shell environment:\n' +
    '  export GITHUB_APP_ID="<app-id>"\n' +
    '  export GITHUB_APP_PRIVATE_KEY_PATH="<absolute-path-to-existing-pem>"\n';
  if (result.keyWarning) text += `WARNING: ${result.keyWarning}\n`;
  return text + 'One GitHub App is used for all worktrees; environment settings do not grant policy.\n';
}
