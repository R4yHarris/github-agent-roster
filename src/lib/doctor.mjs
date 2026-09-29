import { lstatSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { resolveContractsPath } from './paths.mjs';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));

function regularFile(file, inspect) {
  try {
    const status = inspect(file);
    return status.isFile() && !status.isSymbolicLink();
  } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw error;
  }
}

export function checkDoctor({
  cwd = process.cwd(),
  installationRoot = rosterRoot,
  env = process.env,
  nodeVersion = process.version,
  inspect = lstatSync,
  contractsResolver = resolveContractsPath,
} = {}) {
  const version = /^v?(\d+)(?:\.|$)/.exec(nodeVersion);
  const nodeMajor = version ? Number(version[1]) : NaN;
  let contracts = false;
  let contractsReason;
  try {
    contractsResolver({ repoRoot: installationRoot, cwd, env });
    contracts = true;
  } catch (error) {
    if (error instanceof TypeError) contractsReason = 'invalid GITHUB_AGENT_CONTRACTS override';
    else if (error instanceof Error && error.message.startsWith(
      'github-agent-contracts is required: no scripts/agent-pr.mjs file found')) {
      contractsReason = 'publisher not found; initialize the submodule';
    } else throw error;
  }
  const checks = [
    { name: 'Node.js >=20', ok: Number.isSafeInteger(nodeMajor) && nodeMajor >= 20 },
    { name: 'contracts agent-pr.mjs', ok: contracts, ...(!contracts ? { reason: contractsReason } : {}) },
    { name: 'GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH present',
      ok: [env.GITHUB_APP_ID, env.GITHUB_APP_PRIVATE_KEY_PATH].every((value) =>
        typeof value === 'string' && value.trim().length > 0) },
    { name: 'agent-policy.yml', ok: regularFile(join(cwd, 'agent-policy.yml'), inspect) },
    { name: '.github/workflows/check-agent-trailers.yml',
      ok: regularFile(join(cwd, '.github', 'workflows', 'check-agent-trailers.yml'), inspect) },
  ];
  return { ok: checks.every((check) => check.ok), checks };
}

export function formatDoctor(result) {
  if (!Array.isArray(result?.checks) || result.checks.length !== 5) {
    throw new TypeError('Expected five doctor checks');
  }
  return `${result.checks.map((check) =>
    `${check.ok ? 'OK' : 'FAIL'} ${check.name}${check.reason ? `: ${check.reason}` : ''}`).join('\n')}\n`;
}
