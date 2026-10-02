import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { stripVTControlCharacters } from 'node:util';
import { resolveContractsPath } from '../lib/paths.mjs';

export const PRODUCT = 'github-agent-roster';

const colors = { label: '\x1b[96m', white: '\x1b[97m', warn: '\x1b[93m', reset: '\x1b[0m' };
const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));
const clean = (value) => stripVTControlCharacters(String(value ?? '-')).replace(/[\x00-\x1f\x7f]/g, '').trim() || '-';
const paint = (value, color, enabled) => (enabled ? `${colors[color]}${value}${colors.reset}` : String(value));

function packageVersion(root) {
  const parsed = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
  if (typeof parsed?.version !== 'string' || !parsed.version.trim()) throw new TypeError('package.json has no version');
  return parsed.version.trim();
}

export function shellName(env = process.env) {  if (typeof env.SHELL === 'string' && env.SHELL.trim()) return basename(env.SHELL).replace(/\.exe$/i, '').toLowerCase();
  if (typeof env.PSModulePath === 'string' && env.PSModulePath.trim()) return 'powershell';
  if (typeof env.ComSpec === 'string' && env.ComSpec.trim()) return basename(env.ComSpec).replace(/\.exe$/i, '').toLowerCase();
  return '-';
}

function contractsVersion({ env, cwd }) {
  const contracts = resolveContractsPath({ env, cwd, repoRoot: rosterRoot });
  const described = execFileSync('git', ['describe', '--tags', '--always'],
    { cwd: contracts, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], timeout: 1000 }).trim();
  if (!described) throw new Error('git describe returned no tag');
  return described;
}

async function check(warnings, name, read) {
  try {
    const value = await read();
    return value === undefined || value === null || value === '' ? '-' : value;
  } catch (error) {
    warnings.push(`${name} check failed: ${clean(error?.message)}`);
    return '-';
  }
}

export async function collectBannerFacts({
  env = process.env,
  cwd = process.cwd(),
  branch,
  llm = {},
  version,
  services = {},
} = {}) {
  const warnings = [];
  const facts = {
    version: await check(warnings, 'version', () => version ?? packageVersion(rosterRoot)),
    node: await check(warnings, 'node', () => process.versions.node.split('.')[0]),
    shell: await check(warnings, 'shell', () => shellName(env)),
    cwd: await check(warnings, 'repository', () => cwd),
    branch: await check(warnings, 'branch', () => branch),
    model: await check(warnings, 'model', () => llm.model),
    host: await check(warnings, 'host', () => (llm.base_url ? new URL(llm.base_url).host : undefined)),
    contracts: await check(warnings, 'contracts', () => contractsVersion({ env, cwd })),
    endpoint: '-',
    update: 'skipped',
  };
  if (typeof services.probeEndpoint === 'function' || env.ROSTER_BANNER_CHECKS === 'on') {
    facts.endpoint = await check(warnings, 'endpoint', async () => {
      const probe = services.probeEndpoint ?? defaultProbeEndpoint;
      return (await probe({ llm, env })) ? 'ok' : 'unreachable';
    });
  }
  if (typeof services.latestRelease === 'function' || env.ROSTER_BANNER_CHECKS === 'on') {
    facts.update = await check(warnings, 'update', async () => {
      const read = services.latestRelease ?? defaultLatestRelease;
      const latest = await read({ env });
      if (!latest) return 'skipped';
      return clean(latest) === facts.version ? 'current' : clean(latest);
    });
  }
  facts.warnings = warnings;
  return facts;
}

async function defaultProbeEndpoint({ llm }) {
  if (!llm.base_url) throw new TypeError('no endpoint is configured');
  const response = await fetch(new URL('models', `${llm.base_url.replace(/\/*$/, '')}/`), {
    signal: AbortSignal.timeout(1000),
  });
  return response.ok;
}

async function defaultLatestRelease() {
  const response = await fetch('https://api.github.com/repos/R4yHarris/github-agent-roster/releases/latest', {
    headers: { accept: 'application/vnd.github+json' },
    signal: AbortSignal.timeout(1000),
  });
  if (!response.ok) return null;
  const parsed = await response.json();
  return typeof parsed?.tag_name === 'string' ? parsed.tag_name.replace(/^v/, '') : null;
}

export function formatBanner(facts, { color = true } = {}) {
  const value = (name) => paint(clean(facts?.[name]), 'white', color);
  const warnings = Array.isArray(facts?.warnings) ? facts.warnings : [];
  const lines = [
    `${paint(PRODUCT, 'label', color)}  ${value('version')}`,
    `${paint('node', 'label', color)} ${value('node')} \u00b7 ${value('shell')} \u00b7 ${value('cwd')} \u00b7 ${value('branch')}`,
    `${paint('endpoint', 'label', color)} ${value('endpoint')} \u00b7 ${value('model')} \u00b7 ${value('host')}`,
    `${paint('contracts', 'label', color)} ${value('contracts')} \u00b7 ` +
      `${paint('update', 'label', color)} ${value('update')} \u00b7 ` +
      `${paint('warnings', 'label', color)} ${paint(warnings.length || 'none', warnings.length ? 'warn' : 'white', color)}`,
    ...warnings.map((warning) => paint(clean(warning), 'warn', color)),
  ];
  return `${lines.join('\n')}\n`;
}
