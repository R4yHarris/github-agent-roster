import path from 'node:path';
import { formatConfig, loadConfig, parseConfig, readConfigFile, resolveConfigRoot } from '../lib/config.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';

export function privateConfigPath(options) {
  return path.join(resolveConfigRoot(options), '.roster', 'config.yml');
}

export function publicConfig({ cwd, repoRoot, env = process.env }) {
  const file = privateConfigPath({ cwd, repoRoot });
  let config;
  let defaults = false;
  try { config = parseConfig(readConfigFile(file)); }
  catch (error) {
    if (error.code !== 'ENOENT') throw error;
    config = loadConfig({ cwd, repoRoot });
    defaults = true;
  }
  const text = redactEvidence(formatConfig(config), { env, apiKeyEnv: config.llm.api_key_env })
    .replace(/"(?:\\.|[^"\\])*\.pem(?:\\.|[^"\\])*"/gi, '"[redacted PEM path]"');
  return (defaults ? 'Private config has not been written; showing installed defaults.\n' : '') + text;
}

export function configSetting(key, value, { env = process.env, apiKeyEnv = 'ROSTER_API_KEY' } = {}) {
  if (typeof value !== 'string' || /\.pem\b|https?:\/\/|\bagent-policy\b/i.test(value) ||
      redactEvidence(value, { env, apiKeyEnv }) !== value) {
    throw new TypeError('Config set refuses secrets, PEM paths, endpoints, and policy.');
  }
  if (['effort', 'llm.effort'].includes(key)) {
    if (!['l', 'm', 'h', 'x', 'none'].includes(value)) throw new TypeError('Config effort must be l, m, h, x, or none.');
    return { field: 'effort', value };
  }
  if (['context', 'context-budget', 'context_budget', 'context.budget', 'context_chars', 'seat.context_chars'].includes(key)) {
    if (!/^[1-9]\d*$/.test(value) || !Number.isSafeInteger(Number(value))) {
      throw new TypeError('Config context budget must be a positive safe integer.');
    }
    return { field: 'context_budget', value };
  }
  if (['statusbar', 'debug'].includes(key)) {
    if (!['on', 'off', 'true', 'false'].includes(value)) throw new TypeError('Config session toggle must be on or off.');
    return { field: key, value: ['on', 'true'].includes(value) };
  }
  throw new TypeError('Config set allows only effort, context budget, statusbar, and debug.');
}
