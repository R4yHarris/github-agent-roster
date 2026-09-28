import { statSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));

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
