import { randomBytes } from 'node:crypto';
import { promises as fs, lstatSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { TextDecoder } from 'node:util';
import { ensureLocalPath, resolveProjectRoot, RETENTION_POLICIES } from './paths.mjs';
import { STATE_SCOPES, resolveStateDir } from './repo-state.mjs';
import { validateRequestTimeout } from '../llm/request.mjs';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));
const profileNames = ['vllm-local', 'ollama', 'lmstudio', 'openai'];
const defaultProfiles = {
  'vllm-local': {
    base_url: 'http://127.0.0.1:8000/v1', api_key_env: 'ROSTER_API_KEY', api_key_optional: 'true',
  },
  ollama: { base_url: 'http://127.0.0.1:11434/v1', api_key_env: 'ROSTER_API_KEY' },
  lmstudio: { base_url: 'http://127.0.0.1:1234/v1', api_key_env: 'ROSTER_API_KEY' },
  openai: { base_url: 'https://api.openai.com/v1', api_key_env: 'OPENAI_API_KEY' },
};
const fields = {
  llm: ['base_url', 'model', 'api_key_env', 'effort', 'effort_override', 'context_max', 'profile', 'api_key_optional', 'provider', 'request_timeout_ms', 'served_model_label', 'max_requests'],
  planner: ['turn_budget', 'critic_profile'],
  seat: ['id', 'principal', 'turn_budget', 'tools', 'context_chars', 'scope_expansion', 'max_attempts'],
  paths: ['memory', 'skills', 'asks', 'worktrees'],
  publish: ['enabled'],
  tools: ['internet', 'run_test'],
  reviewer: ['required'],
  review: ['required'],
  loop: ['turns'],
  context: ['budget'],
};
const availableTools = ['read_file', 'write_file', 'edit_file', 'glob_files', 'list_dir', 'run_test', 'run_command', 'search_text', 'web_search', 'web_fetch'];

export class ConfigError extends Error {}

function invalid(message) {
  throw new ConfigError(`Invalid roster config: ${message}`);
}

function uncomment(line) {
  let quoted = false;
  let escaped = false;
  for (let index = 0; index < line.length; index += 1) {
    const char = line[index];
    if (quoted && char === '\\' && !escaped) {
      escaped = true;
      continue;
    }
    if (char === '"' && !escaped) quoted = !quoted;
    if (!quoted && char === '#' && (index === 0 || /\s/.test(line[index - 1]))) {
      return line.slice(0, index).trimEnd();
    }
    escaped = false;
  }
  return line.trimEnd();
}

function stringValue(value, name) {
  if (value.startsWith('"')) {
    try {
      const decoded = JSON.parse(value);
      if (typeof decoded === 'string') return decoded;
    } catch {
      invalid(`${name} must be a plain or double-quoted string`);
    }
  }
  if (!/^[A-Za-z0-9_./:@\\-]+$/.test(value)) {
    invalid(`${name} must be a plain or double-quoted string`);
  }
  return value;
}

function integerValue(value, name) {
  if (!/^(?:0|[1-9]\d*)$/.test(value) || !Number.isSafeInteger(Number(value))) {
    invalid(`${name} must be a nonnegative safe integer`);
  }
  return Number(value);
}

function booleanValue(value, name) {
  if (!['true', 'false'].includes(value)) invalid(`${name} must be true or false`);
  return value === 'true';
}

function relativePath(value, name) {
  const segments = value.split(/[\\/]/);
  if (path.isAbsolute(value) || !segments.length ||
      segments.some((part) => !/^[A-Za-z0-9_.-]+$/.test(part) || part === '.' || part === '..' ||
        part.toLowerCase() === '.git' || part.toLowerCase() === '.env')) {
    invalid(`${name} must be a relative path inside the roster repository`);
  }
  return segments.join(path.sep);
}

function apiKeyName(value, name) {
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(value) || /^GITHUB_APP_/i.test(value)) {
    invalid(`${name} must name an LLM environment variable, not a GitHub App credential`);
  }
  return value;
}

export function validateBaseUrl(value, name = 'llm.base_url') {
  let url;
  try {
    url = new URL(value);
  } catch {
    invalid(`${name} must be an HTTP(S) URL without credentials, query, or fragment`);
  }
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password ||
      url.search || url.hash) {
    invalid(`${name} must be an HTTP(S) URL without credentials, query, or fragment`);
  }
  return value;
}

export function parseConfig(source) {
  if (typeof source !== 'string' || Buffer.byteLength(source, 'utf8') > 65_536) {
    invalid('expected UTF-8 text of at most 64 KiB');
  }
  const config = { llm: {}, profiles: {}, planner: {}, seat: {}, paths: {},
    publish: {}, tools: {}, reviewer: {}, review: {}, loop: {}, context: {} };
  const roots = new Set();
  let section;
  let profile;
  for (const [index, original] of source.replace(/\r\n/g, '\n').split('\n').entries()) {
    if (/[\x00-\x09\x0b-\x1f\x7f]/.test(original)) invalid(`unsupported character on line ${index + 1}`);
    const line = uncomment(original);
    if (!line) continue;
    if (line.startsWith('    ')) {
      const match = /^    (base_url|api_key_env|api_key_optional): (.+)$/.exec(line);
      if (!match || section !== 'profiles' || !profile ||
          Object.hasOwn(config.profiles[profile], match[1])) {
        invalid(`unsupported or duplicate profile field on line ${index + 1}`);
      }
      config.profiles[profile][match[1]] = match[2];
      continue;
    }
    if (line.startsWith('  ')) {
      if (section === 'profiles') {
        const match = /^  (vllm-local|ollama|lmstudio|openai):$/.exec(line);
        if (!match || Object.hasOwn(config.profiles, match[1])) {
          invalid(`unsupported or duplicate profile on line ${index + 1}`);
        }
        profile = match[1];
        config.profiles[profile] = {};
        continue;
      }
      const match = /^  ([a-z_]+): (.+)$/.exec(line);
      if (!match || !section || !fields[section].includes(match[1]) ||
          Object.hasOwn(config[section], match[1])) {
        invalid(`unsupported or duplicate field on line ${index + 1}`);
      }
      config[section][match[1]] = match[2];
      continue;
    }
    const match = /^([a-z_]+):(?: (.*))?$/.exec(line);
    if (!match || roots.has(match[1]) || !['schema', 'profiles', ...Object.keys(fields)].includes(match[1])) {
      invalid(`unsupported or duplicate field on line ${index + 1}`);
    }
    roots.add(match[1]);
    section = match[1] === 'schema' ? undefined : match[1];
    profile = undefined;
    if (match[1] === 'schema') {
      if (match[2] !== '1') invalid('schema must be 1');
    } else if (match[2] !== undefined) {
      invalid(`${match[1]} must be a mapping`);
    }
  }
  if (!roots.has('schema') ||
      ['llm', 'seat', 'paths'].some((name) =>
        !roots.has(name) || fields[name].some((field) =>
          !(name === 'llm' && ['profile', 'api_key_optional', 'provider', 'request_timeout_ms', 'effort_override', 'served_model_label', 'max_requests'].includes(field)) &&
          !(name === 'seat' && ['context_chars', 'scope_expansion', 'max_attempts'].includes(field)) && !Object.hasOwn(config[name], field))) ||
      (roots.has('planner') && !Object.hasOwn(config.planner, 'turn_budget'))) {
    invalid('schema, llm, seat, paths, and optional planner must contain every documented field');
  }

  const llm = config.llm;
  llm.base_url = stringValue(llm.base_url, 'llm.base_url');
  llm.model = stringValue(llm.model, 'llm.model');
  llm.api_key_env = apiKeyName(stringValue(llm.api_key_env, 'llm.api_key_env'), 'llm.api_key_env');
  llm.effort = stringValue(llm.effort, 'llm.effort');
  if (Object.hasOwn(llm, 'effort_override')) {
    llm.effort_override = stringValue(llm.effort_override, 'llm.effort_override');
    if (!['l', 'm', 'h', 'x', 'none'].includes(llm.effort_override)) invalid('llm.effort_override must be l, m, h, x, or none');
  }
  llm.context_max = integerValue(llm.context_max, 'llm.context_max');
  if (Object.hasOwn(llm, 'max_requests')) {
    llm.max_requests = integerValue(llm.max_requests, 'llm.max_requests');
    if (llm.max_requests < 1 || llm.max_requests > 10000) invalid('llm.max_requests must be an integer from 1 to 10000');
  }
  if (Object.hasOwn(llm, 'request_timeout_ms')) {
    llm.request_timeout_ms = integerValue(llm.request_timeout_ms, 'llm.request_timeout_ms');
    try {
      validateRequestTimeout(llm.request_timeout_ms);
    } catch (error) {
      if (!(error instanceof TypeError)) throw error;
      invalid(error.message);
    }
  }
  llm.profile = Object.hasOwn(llm, 'profile') ? stringValue(llm.profile, 'llm.profile') : '';
  if (Object.hasOwn(llm, 'served_model_label')) {
    llm.served_model_label = stringValue(llm.served_model_label, 'llm.served_model_label');
    if (!['trust', 'ignore'].includes(llm.served_model_label)) invalid('llm.served_model_label must be trust or ignore');
  }
  if (Object.hasOwn(llm, 'api_key_optional')) {
    llm.api_key_optional = booleanValue(llm.api_key_optional, 'llm.api_key_optional');
  }
  if (Object.hasOwn(llm, 'provider')) {
    llm.provider = stringValue(llm.provider, 'llm.provider');
    if (!['vllm', 'github-copilot', 'anthropic', 'openai', 'local', 'other'].includes(llm.provider)) {
      invalid('llm.provider must name a supported model backend');
    }
  }
  if (llm.profile && !profileNames.includes(llm.profile)) {
    invalid('llm.profile must be vllm-local, ollama, lmstudio, openai, or empty');
  }
  const profiles = roots.has('profiles') ? config.profiles :
    Object.fromEntries(profileNames.map((name) => [name, { ...defaultProfiles[name] }]));
  if (roots.has('profiles') && !Object.hasOwn(profiles, 'vllm-local')) {
    profiles['vllm-local'] = { ...defaultProfiles['vllm-local'] };
  }
  if (profileNames.some((name) => !Object.hasOwn(profiles, name) ||
      ['base_url', 'api_key_env', ...(name === 'vllm-local' ? ['api_key_optional'] : [])]
        .some((field) => !Object.hasOwn(profiles[name], field)))) {
    invalid('profiles need base_url and api_key_env for every named profile, plus api_key_optional for vllm-local');
  }
  for (const name of profileNames) {
    profiles[name].base_url = validateBaseUrl(stringValue(profiles[name].base_url, `profiles.${name}.base_url`),
      `profiles.${name}.base_url`);
    profiles[name].api_key_env = apiKeyName(
      stringValue(profiles[name].api_key_env, `profiles.${name}.api_key_env`),
      `profiles.${name}.api_key_env`);
    if (Object.hasOwn(profiles[name], 'api_key_optional')) {
      profiles[name].api_key_optional = booleanValue(
        profiles[name].api_key_optional, `profiles.${name}.api_key_optional`);
    }
    Object.freeze(profiles[name]);
  }
  if (llm.profile) {
    if (llm.base_url && llm.profile !== 'vllm-local') invalid('choose either llm.profile or llm.base_url');
    llm.base_url ||= profiles[llm.profile].base_url;
    llm.api_key_env = profiles[llm.profile].api_key_env;
    if (!Object.hasOwn(llm, 'api_key_optional') &&
        Object.hasOwn(profiles[llm.profile], 'api_key_optional')) {
      llm.api_key_optional = profiles[llm.profile].api_key_optional;
    }
  }
  if (!['l', 'm', 'h', 'x', 'none'].includes(llm.effort)) invalid('llm.effort must be l, m, h, x, or none');
  if (llm.model && !/^[A-Za-z0-9._:/-]+$/.test(llm.model)) invalid('llm.model must be a model name without whitespace');
  if (llm.base_url) {
    validateBaseUrl(llm.base_url, 'llm.base_url');
  }

  const seat = config.seat;
  seat.id = stringValue(seat.id, 'seat.id');
  seat.principal = stringValue(seat.principal, 'seat.principal');
  seat.turn_budget = integerValue(seat.turn_budget, 'seat.turn_budget');
  seat.context_chars = Object.hasOwn(seat, 'context_chars')
    ? integerValue(seat.context_chars, 'seat.context_chars') : 200000;
  if (seat.context_chars < 1) invalid('seat.context_chars must be positive');
  seat.scope_expansion = Object.hasOwn(seat, 'scope_expansion')
    ? integerValue(seat.scope_expansion, 'seat.scope_expansion') : 3;
  if (seat.scope_expansion < 0 || seat.scope_expansion > 16) invalid('seat.scope_expansion must be 0-16 files (0 is strict scope)');
  if (seat.id !== 'coder' || seat.principal !== 'coder' || seat.turn_budget < 1 || seat.turn_budget > 10000) {
    invalid('seat must be coder with principal coder and turn_budget between 1 and 10000');
  }
  const toolList = /^\[([^\[\]]*)\]$/.exec(seat.tools);
  if (!toolList) invalid('seat.tools must be an inline list of builtin tools');
  seat.tools = toolList[1].split(',').map((tool) => tool.trim());
  if (!seat.tools.length || new Set(seat.tools).size !== seat.tools.length ||
      seat.tools.some((tool) => !availableTools.includes(tool))) {
    invalid('seat.tools may only contain distinct read_file, write_file, edit_file, glob_files, list_dir, run_test, run_command, search_text, web_search, web_fetch tools');
  }
  seat.tools = Object.freeze(seat.tools);

  const planner = config.planner;
  planner.turn_budget = roots.has('planner')
    ? integerValue(planner.turn_budget, 'planner.turn_budget') : 1;
  if (planner.turn_budget < 1 || planner.turn_budget > 10000) {
    invalid('planner.turn_budget must be between 1 and 10000');
  }
  if (planner.critic_profile !== undefined && !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(planner.critic_profile)) {
    invalid('planner.critic_profile must name a fleet profile');
  }
  if (roots.has('loop')) {
    config.loop.turns = integerValue(config.loop.turns, 'loop.turns');
    if (config.loop.turns < 1 || config.loop.turns > 10000) invalid('loop.turns must be between 1 and 10000');
    seat.turn_budget = config.loop.turns;
  }
  seat.max_attempts = seat.max_attempts === undefined ? 3 : integerValue(seat.max_attempts, 'seat.max_attempts');
  if (seat.max_attempts < 1 || seat.max_attempts > 16) invalid('seat.max_attempts must be between 1 and 16');
  if (roots.has('context')) {
    config.context.budget = integerValue(config.context.budget, 'context.budget');
    if (config.context.budget < 1) invalid('context.budget must be positive');
    seat.context_chars = config.context.budget;
  }

  for (const name of fields.paths) {
    config.paths[name] = relativePath(stringValue(config.paths[name], `paths.${name}`), `paths.${name}`);
  }
  if (!config.paths.memory.endsWith('.jsonl')) invalid('paths.memory must be a .jsonl file');
  for (const [section, defaults] of Object.entries({
    publish: { enabled: true }, tools: { internet: false, run_test: true }, reviewer: { required: true },
  })) {
    for (const [field, fallback] of Object.entries(defaults)) {
      config[section][field] = Object.hasOwn(config[section], field)
        ? booleanValue(config[section][field], `${section}.${field}`) : fallback;
    }
  }
  if (roots.has('review')) {
    config.review.required = booleanValue(config.review.required, 'review.required');
    if (roots.has('reviewer') && config.review.required !== config.reviewer.required) {
      invalid('review.required and legacy reviewer.required disagree');
    }
    config.reviewer.required = config.review.required;
  }
  return Object.freeze({
    schema: 1,
    llm: Object.freeze(llm),
    profiles: Object.freeze(profiles),
    planner: Object.freeze(planner),
    seat: Object.freeze(seat),
    paths: Object.freeze(config.paths),
    publish: Object.freeze(config.publish),
    tools: Object.freeze(config.tools),
    reviewer: Object.freeze(config.reviewer),
    ...(roots.has('review') ? { review: Object.freeze(config.review) } : {}),
    ...(roots.has('loop') ? { loop: Object.freeze(config.loop) } : {}),
    ...(roots.has('context') ? { context: Object.freeze(config.context) } : {}),
  });
}

export function readConfigFile(file) {
  const directory = lstatSync(path.dirname(file));
  if (!directory.isDirectory() || directory.isSymbolicLink()) {
    invalid('expected a non-symlink config directory');
  }
  const stat = lstatSync(file);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 65_536) {
    invalid('expected a regular, non-symlink file of at most 64 KiB');
  }
  const bytes = readFileSync(file);
  try {
    return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch {
    invalid('config file must be UTF-8');
  }
}

export function formatConfig(config) {
  const scalar = (value) => Array.isArray(value) ? `[${value.join(', ')}]`
    : typeof value === 'string' ? JSON.stringify(value) : String(value);
  const mapping = (values, indent = '') => Object.entries(values).map(([name, value]) =>
    value && typeof value === 'object' && !Array.isArray(value)
      ? `${indent}${name}:\n${mapping(value, `${indent}  `)}`
      : `${indent}${name}: ${scalar(value)}\n`).join('');
  // Fleet admission fields are per-run routing state from withFleetProfile, never private settings.
  const { fleet_profile: _fleet, concurrency: _concurrency, ...llm } = config.llm ?? {};
  const source = '# Private settings. No credentials belong in this file.\n' +
    mapping(config.llm ? { ...config, llm } : config);
  parseConfig(source);
  return source;
}

// The private config is SHARED repo state: it lives once at the repo-common
// root and every linked worktree reads the same bytes through the repo-state
// API instead of constructing its own private path. `configStatePath` stays
// synchronous because loadConfig/setConfigValue are synchronous public APIs.
export function configStatePath(repoRoot) {
  return resolveStateDir({ scope: STATE_SCOPES.SHARED, repoRoot });
}

export function resolveConfigRoot({ repoRoot = rosterRoot, cwd } = {}) {
  if (cwd !== undefined) {
    const project = resolveProjectRoot(cwd);
    const local = path.join(configStatePath(project).legacyRoot, 'config.yml');
    if (lstatSync(local, { throwIfNoEntry: false })) return project;
  }
  return path.resolve(repoRoot);
}

export function loadConfig({ repoRoot = rosterRoot, cwd } = {}) {
  const configRoot = resolveConfigRoot({ repoRoot, cwd });
  const local = path.join(configStatePath(configRoot).legacyRoot, 'config.yml');
  let source;
  try {
    source = readConfigFile(local);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    source = readConfigFile(path.join(repoRoot, 'roster.config.example.yml'));
  }
  return parseConfig(source);
}

// Pure over an injected env; see docs/STATE.md §8.5 for the keys and defaults.
function retentionWindowKey(scope) {
  return `ROSTER_RETENTION_WINDOW_MS_${scope.toUpperCase()}`;
}

export function resolveRetentionConfig(env = {}) {
  if (typeof env !== 'object' || env === null) {
    throw new ConfigError('resolveRetentionConfig requires an env object');
  }
  const rawOptOut = env.ROSTER_RETENTION_OPT_OUT;
  let optOut = false;
  if (rawOptOut !== undefined) {
    if (rawOptOut === 'true') optOut = true;
    else if (rawOptOut === 'false') optOut = false;
    else throw new ConfigError(
      'ROSTER_RETENTION_OPT_OUT must be "true" or "false" when set');
  }
  const windowsMs = {};
  for (const scope of Object.keys(RETENTION_POLICIES)) {
    const defaultMs = RETENTION_POLICIES[scope].defaultWindowMs;
    const raw = env[retentionWindowKey(scope)];
    if (raw === undefined) {
      windowsMs[scope] = defaultMs;
      continue;
    }
    if (typeof raw !== 'string' || !/^(?:0|[1-9]\d*)$/.test(raw) ||
        !Number.isSafeInteger(Number(raw)) || Number(raw) <= 0) {
      throw new ConfigError(
        `${retentionWindowKey(scope)} must be a positive integer (ms) when set`);
    }
    windowsMs[scope] = Number(raw);
  }
  return Object.freeze({ optOut, windowsMs: Object.freeze(windowsMs) });
}

export function requirePublicationEnabled(config) {
  if (config.publish?.enabled === false) {
    throw new Error('Publishing is disabled by publish.enabled; update the private config or rerun roster onboard');
  }
}

export function withoutLlmKeys(env, config) {
  const clean = { ...env };
  for (const name of [config.llm.api_key_env,
    ...Object.values(config.profiles ?? {}).map((profile) => profile.api_key_env)]) {
    if (typeof name === 'string' && name) delete clean[name];
  }
  return clean;
}

export function isReviewRequired(config) {
  return (config.review?.required ?? config.reviewer?.required) !== false;
}

export async function setConfigValue(field, value, { repoRoot = rosterRoot, cwd } = {}) {
  if (!['model', 'effort', 'context_budget'].includes(field) || typeof value !== 'string' ||
      (field !== 'model' && !value) || /[\r\n\0]/.test(value)) {
    throw new TypeError('Set a single-line model, effort, or context budget value');
  }
  const configRoot = resolveConfigRoot({ repoRoot, cwd });
  const file = path.join(configStatePath(configRoot).legacyRoot, 'config.yml');
  await ensureLocalPath(file, configRoot);
  let source;
  try {
    source = readConfigFile(file);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    source = readConfigFile(path.join(repoRoot, 'roster.config.example.yml'));
  }
  const text = source.replace(/\r\n/g, '\n');
  if (field === 'context_budget') {
    if (!/^[1-9]\d*$/.test(value) || !Number.isSafeInteger(Number(value))) {
      throw new TypeError('Context budget must be a positive safe integer');
    }
    let next = /^  context_chars: [^\n]*$/m.test(text)
      ? text.replace(/^  context_chars: [^\n]*$/m, `  context_chars: ${value}`)
      : text.replace(/^seat:\n/m, `seat:\n  context_chars: ${value}\n`);
    if (/^context:\n/m.test(next)) next = next.replace(/^  budget: [^\n]*$/m, `  budget: ${value}`);
    return writePrivateConfig(next, { repoRoot: configRoot });
  }
  const line = new RegExp(`^  ${field}: [^\\n]*$`, 'm');
  if (!line.test(text)) invalid(`llm.${field} is missing`);
  let next = text.replace(line, `  ${field}: ${field === 'model' ? JSON.stringify(value) : value}`);
  if (field === 'effort') {
    const override = /^  effort_override: [^\n]*$/m;
    next = override.test(next) ? next.replace(override, `  effort_override: ${value}`)
      : next.replace(/^  effort: [^\n]*$/m, (entry) => `${entry}\n  effort_override: ${value}`);
  }
  return writePrivateConfig(next, { repoRoot: configRoot });
}

export async function writePrivateConfig(source, { repoRoot = rosterRoot, expectedSource } = {}) {
  const config = parseConfig(source);
  const file = path.join(configStatePath(repoRoot).legacyRoot, 'config.yml');
  await ensureLocalPath(file, repoRoot);
  let previous = null;
  try {
    previous = readConfigFile(file);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  if (expectedSource !== undefined && previous !== expectedSource) {
    throw new Error('Private config changed during onboarding; refusing to overwrite it');
  }
  const directory = path.dirname(file);
  await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  await ensureLocalPath(file, repoRoot);
  const temporary = path.join(directory, `config.yml.${randomBytes(8).toString('hex')}.tmp`);
  try {
    await fs.writeFile(temporary, source, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
    await fs.rename(temporary, file);
  } finally {
    await fs.rm(temporary, { force: true });
  }
  return config;
}
