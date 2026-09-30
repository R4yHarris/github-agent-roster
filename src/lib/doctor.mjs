import { lstatSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { resolvePublishModel } from '../metrics/run.mjs';
import { loadConfig } from './config.mjs';
import { resolveContractsPath, resolveProjectRoot } from './paths.mjs';

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
  const projectRoot = resolveProjectRoot(cwd);
  const version = /^v?(\d+)(?:\.|$)/.exec(nodeVersion);
  const nodeMajor = version ? Number(version[1]) : NaN;
  let contracts = false;
  let contractsReason;
  try {
    contractsResolver({ repoRoot: installationRoot, cwd, env });
    contracts = regularFile(join(installationRoot, 'vendor', 'github-agent-contracts',
      'scripts', 'agent-pr.mjs'), inspect);
    if (!contracts) contractsReason = 'vendor publisher not found; initialize the submodule';
  } catch (error) {
    if (error instanceof TypeError) contractsReason = 'invalid GITHUB_AGENT_CONTRACTS override';
    else if (error instanceof Error && error.message.startsWith(
      'github-agent-contracts is required: no scripts/agent-pr.mjs file found')) {
      contractsReason = 'publisher not found; initialize the submodule';
    } else throw error;
  }
  let config;
  let configReason;
  try {
    config = loadConfig({ repoRoot: installationRoot, cwd: projectRoot });
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    configReason = 'config could not be read or validated; rerun roster onboard or fix the private config';
  }
  const publishExpected = config?.publish.enabled !== false;
  const modelExpected = publishExpected || Boolean(config?.llm.base_url);
  let appReady = false;
  let appReason;
  if (publishExpected) {
    appReady = [env.GITHUB_APP_ID, env.GITHUB_APP_PRIVATE_KEY_PATH].every((value) =>
      typeof value === 'string' && value.trim().length > 0);
    if (appReady) {
      try {
        appReady = regularFile(resolve(projectRoot, env.GITHUB_APP_PRIVATE_KEY_PATH), statSync);
        if (!appReady) appReason = 'App private-key file is missing or not regular';
      } catch (error) {
        if (!(error instanceof Error)) throw error;
        appReady = false;
        appReason = 'App private-key file could not be checked';
      }
    }
  }
  let modelReady = false;
  if (!configReason && regularFile(join(projectRoot, '.roster', 'config.yml'), inspect)) {
    try {
      resolvePublishModel({ config, env: {} });
      modelReady = true;
    } catch (error) {
      if (!(error instanceof Error)) throw error;
      configReason = 'saved model is missing or invalid; rerun roster onboard';
    }
  }
  const publishingCheck = (name, ok, reason) => ({
    name, ok: !publishExpected || ok, ...(!publishExpected ? { skipped: true } : {}),
    ...(publishExpected && !ok && reason ? { reason } : {}),
  });
  const checks = [
    { name: 'Node.js >=20', ok: Number.isSafeInteger(nodeMajor) && nodeMajor >= 20 },
    { name: 'contracts agent-pr.mjs', ok: contracts, ...(!contracts ? { reason: contractsReason } : {}) },
    publishingCheck('GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY_PATH present', appReady, appReason),
    publishingCheck('agent-policy.yml',
      publishExpected && regularFile(join(projectRoot, 'agent-policy.yml'), inspect)),
    publishingCheck('.github/workflows/check-agent-trailers.yml',
      publishExpected && regularFile(join(projectRoot, '.github', 'workflows', 'check-agent-trailers.yml'), inspect)),
    { name: '.roster/config.yml model', ok: !modelExpected || modelReady,
      ...(!modelExpected ? { skipped: true } : {}),
      ...(modelExpected && !modelReady ? {
        reason: configReason ?? 'private config with a real model is required; run roster onboard',
      } : {}) },
  ];
  return { ok: checks.every((check) => check.ok), checks };
}

export function formatDoctor(result) {
  if (!Array.isArray(result?.checks) || result.checks.length !== 6 ||
      result.checks.some((check) => typeof check?.name !== 'string' || typeof check.ok !== 'boolean')) {
    throw new TypeError('Expected six doctor checks');
  }
  return `${result.checks.map((check) =>
    `${check.skipped ? 'SKIP' : check.ok ? 'OK' : 'FAIL'} ${check.name}${check.reason ? `: ${check.reason}` : ''}`).join('\n')}\n`;
}
