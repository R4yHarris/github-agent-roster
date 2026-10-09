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

function canonicalRemoteUrl(remoteUrl) {
  const trimmed = remoteUrl.trim();
  let host, port = '', repoPath;
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed)) {
    let url;
    try {
      url = new URL(trimmed);
    } catch (cause) {
      throw new RepoIdentityError('Invalid repository remote URL.', { code: 'E_IDENTITY_INPUT', cause });
    }
    if (!['https:', 'http:', 'ssh:', 'git:', 'file:'].includes(url.protocol) || url.search || url.hash) {
      throw new RepoIdentityError('Unsupported repository remote URL or selector.',
        { code: 'E_IDENTITY_INPUT' });
    }
    host = url.hostname.toLowerCase();
    port = url.protocol === 'ssh:' && url.port === '22' ? '' : url.port;
    repoPath = url.pathname;
    if (url.protocol === 'file:') return `file:${host}${repoPath}`;
  } else {
    const ssh = !path.isAbsolute(trimmed) && !/^[a-z]:[\\/]/i.test(trimmed)
      ? trimmed.match(/^(?:[^@/:]+@)?(\[[^\]]+\]|[^/:]+):(.+)$/)
      : null;
    if (!ssh) return `local:${trimmed}`;
    host = ssh[1].toLowerCase();
    repoPath = ssh[2];
  }
  // GitHub paths are case-insensitive; arbitrary Git servers need not be.
  repoPath = repoPath.replace(/^\/+/, '').replace(/\/+$/, '').replace(/\.git$/i, '');
  if (host === 'github.com') repoPath = repoPath.toLowerCase();
  if (!host || !repoPath) {
    throw new RepoIdentityError('Repository remote requires a host and repository path.',
      { code: 'E_IDENTITY_INPUT' });
  }
  return `${host}${port ? `:${port}` : ''}/${repoPath}`;
}

/**
 * Derive a repo identity from the origin remote URL, returned as a hash.
 * The identity is invariant under clone path, checkout location, and git
 * common dir, so a re-clone or a moved checkout of the same repository
 * resolves to the same identity. The raw remote URL and git dir are never
 * returned or persisted in plaintext, so nothing secret-looking is ever
 * written or logged; tests feed a non-credential sentinel remote and assert
 * it never surfaces.
 *
 * Checkout paths are deliberately excluded from the hash.
 */
export function identityHash({ remoteUrl, algorithm = 'sha256' } = {}) {
  if (typeof remoteUrl !== 'string' || remoteUrl.trim() === '') {
    throw new RepoIdentityError(
      'repo identity requires the origin remote URL.',
      { code: 'E_IDENTITY_INPUT' });
  }
  const digest = createHash(algorithm);
  digest.update(`remote-url:${canonicalRemoteUrl(remoteUrl)}\n`);
  return `${algorithm}-${digest.digest('hex')}`;
}

/**
 * Resolve the current repo identity by asking git for the common dir (to
 * confirm the caller is inside a repository) and the configured origin URL.
 * Fails closed when git metadata is unreadable. The identity itself is
 * derived from the origin remote URL only, so it is stable across re-clones
 * and moved checkouts of the same repository.
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

  // Relative local origins are relative to the checkout, not the machine cwd.
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(remoteUrl) &&
      !/^(?:[^@/:]+@)?[^/:]+:.+/.test(remoteUrl)) {
    remoteUrl = path.resolve(repoRoot ?? process.cwd(), remoteUrl);
  }
  return identityHash({ remoteUrl });
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
      'Restore the original origin remote or explicitly reinitialize repository state after preserving it. ' +
      'Legacy path-derived history remains untouched and is not automatically reassigned.',
  };
}
