import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { promisify } from 'node:util';

const execute = promisify(execFile);

export class RepoIdentityError extends Error {
  constructor(message, { code, identity, cause } = {}) {
    super(message);
    this.name = 'RepoIdentityError';
    if (code !== undefined) this.code = code;
    if (identity !== undefined) this.identity = identity;
    if (cause !== undefined) this.cause = cause;
  }
}

/**
 * Derive a repo identity from git metadata (git common dir + origin remote),
 * returned as a hash. The raw remote URL and git dir are never returned or
 * persisted in plaintext, so nothing secret-looking is ever written or logged;
 * tests feed a non-credential sentinel remote and assert it never surfaces.
 */
export function identityHash({ gitCommonDir, remoteUrl, algorithm = 'sha256' } = {}) {
  if (typeof gitCommonDir !== 'string' || gitCommonDir.trim() === '' ||
      typeof remoteUrl !== 'string' || remoteUrl.trim() === '') {
    throw new RepoIdentityError(
      'repo identity requires both the git common dir and the origin remote URL.',
      { code: 'E_IDENTITY_INPUT' });
  }
  const digest = createHash(algorithm);
  digest.update(`git-common-dir:${gitCommonDir.trim()}\n`);
  digest.update(`remote-url:${remoteUrl.trim()}\n`);
  return `${algorithm}-${digest.digest('hex')}`;
}

/**
 * Resolve the current repo identity by asking git for the common dir and the
 * configured origin URL. Fails closed when git metadata is unreadable.
 */
export async function resolveRepoIdentity({ repoRoot, run = execute } = {}) {
  const git = (args) => run('git', ['--no-pager', ...args], {
    cwd: repoRoot, encoding: 'utf8', timeout: 10_000,
  });

  let commonDir;
  try {
    commonDir = (await git(['rev-parse', '--git-common-dir'])).stdout.trim();
  } catch (error) {
    throw new RepoIdentityError(
      `could not read the git common dir for ${repoRoot}: ${error.message}. ` +
      'Run inside a git repository or initialize one with `git init`.',
      { code: 'E_GIT_METADATA', cause: error });
  }
  if (commonDir === '') {
    throw new RepoIdentityError('git returned an empty common directory; refusing ambiguous repository identity.',
      { code: 'E_IDENTITY_INPUT' });
  }

  let remoteUrl;
  try {
    remoteUrl = (await git(['config', '--get', 'remote.origin.url'])).stdout.trim();
  } catch (error) {
    throw new RepoIdentityError(
      `could not read the origin remote for ${repoRoot}: ${error.message}. ` +
      'Add one with `git remote add origin <url>`, or record the repository identity manually.',
      { code: 'E_GIT_METADATA', cause: error });
  }

  return identityHash({ gitCommonDir: canonicalCommonDir(repoRoot, commonDir), remoteUrl });
}

// git prints the common dir relative in the main checkout and absolute in linked worktrees;
// one repository must hash to one identity from either place.
function canonicalCommonDir(repoRoot, commonDir) {
  const absolute = path.resolve(repoRoot ?? process.cwd(), commonDir).split(path.sep).join('/');
  return process.platform === 'win32' ? absolute.toLowerCase() : absolute;
}

/** Compare a freshly derived identity against the recorded one. */
export function compareIdentity(recorded, current) {
  if (recorded === current) return { status: 'match' };
  if (recorded === undefined || recorded === null) return { status: 'initialize', identity: current };
  return {
    status: 'mismatch',
    recorded,
    current,
    message:
      'the recorded repository identity does not match this checkout. ' +
      'Either restore the original checkout (same git common dir and origin remote) ' +
      'or delete the state directory to re-initialize it for the new repository.',
  };
}
