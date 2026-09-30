import { promises as fs, lstatSync, statSync } from 'node:fs';
import { join, relative, resolve, sep, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));

export function resolveProjectRoot(cwd = process.cwd()) {
  const start = resolve(cwd);
  let current = start;
  for (;;) {
    const marker = lstatSync(join(current, '.git'), { throwIfNoEntry: false });
    if (marker) {
      if (marker.isSymbolicLink() || (!marker.isFile() && !marker.isDirectory())) {
        throw new Error('Git worktree marker must be a regular file or directory');
      }
      return current;
    }
    const parent = resolve(current, '..');
    if (parent === current) return start;
    current = parent;
  }
}

export function resolveContractsPath({
  env = process.env,
  cwd = process.cwd(),
  repoRoot = rosterRoot,
} = {}) {
  const configuredPath = env.GITHUB_AGENT_CONTRACTS;

  if (configuredPath !== undefined &&
      (typeof configuredPath !== 'string' || configuredPath.trim() === '')) {
    throw new TypeError('GITHUB_AGENT_CONTRACTS must be a non-empty path');
  }

  const candidates = [resolve(repoRoot, 'vendor', 'github-agent-contracts')];
  if (configuredPath !== undefined) {
    candidates.push(resolve(cwd, configuredPath));
  }
  candidates.push(resolve(repoRoot, '..', 'github-agent-contracts'));

  for (const contractsPath of candidates) {
    const publisher = resolve(contractsPath, 'scripts', 'agent-pr.mjs');
    if (statSync(publisher, { throwIfNoEntry: false })?.isFile()) {
      return contractsPath;
    }
  }

  throw new Error(
    `github-agent-contracts is required: no scripts/agent-pr.mjs file found in ${candidates.join(', ')}. ` +
    'Run git submodule update --init --recursive or set GITHUB_AGENT_CONTRACTS to a complete clone.',
  );
}

export async function ensureLocalPath(file, repoRoot) {
  const root = await fs.realpath(repoRoot);
  const target = resolve(file);
  const rest = relative(root, target);
  if (!rest || rest === '..' || rest.startsWith(`..${sep}`) || isAbsolute(rest)) {
    throw new Error('Path must stay inside the roster repository');
  }
  let current = root;
  for (const part of rest.split(sep)) {
    current = join(current, part);
    let entry;
    try {
      entry = await fs.lstat(current);
    } catch (error) {
      if (error.code === 'ENOENT') break;
      throw error;
    }
    if (entry.isSymbolicLink()) throw new Error('Roster path may not contain symlinks');
  }
}
