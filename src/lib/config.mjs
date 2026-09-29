import { lstatSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { TextDecoder } from 'node:util';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));
const fields = {
  llm: ['base_url', 'model', 'api_key_env', 'effort', 'context_max'],
  seat: ['id', 'principal', 'turn_budget', 'tools'],
  paths: ['memory', 'skills', 'asks', 'worktrees'],
};
const availableTools = ['read_file', 'write_file', 'list_dir', 'run_test'];

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

function relativePath(value, name) {
  const segments = value.split(/[\\/]/);
  if (path.isAbsolute(value) || !segments.length ||
      segments.some((part) => !/^[A-Za-z0-9_.-]+$/.test(part) || part === '.' || part === '..' ||
        part.toLowerCase() === '.git' || part.toLowerCase() === '.env')) {
    invalid(`${name} must be a relative path inside the roster repository`);
  }
  return segments.join(path.sep);
}

export function parseConfig(source) {
  if (typeof source !== 'string' || Buffer.byteLength(source, 'utf8') > 65_536) {
    invalid('expected UTF-8 text of at most 64 KiB');
  }
  const config = { llm: {}, seat: {}, paths: {} };
  const roots = new Set();
  let section;
  for (const [index, original] of source.replace(/\r\n/g, '\n').split('\n').entries()) {
    if (/[\x00-\x09\x0b-\x1f\x7f]/.test(original)) invalid(`unsupported character on line ${index + 1}`);
    const line = uncomment(original);
    if (!line) continue;
    if (line.startsWith('  ')) {
      const match = /^  ([a-z_]+): (.+)$/.exec(line);
      if (!match || !section || !fields[section].includes(match[1]) ||
          Object.hasOwn(config[section], match[1])) {
        invalid(`unsupported or duplicate field on line ${index + 1}`);
      }
      config[section][match[1]] = match[2];
      continue;
    }
    const match = /^([a-z_]+):(?: (.*))?$/.exec(line);
    if (!match || roots.has(match[1]) || !['schema', ...Object.keys(fields)].includes(match[1])) {
      invalid(`unsupported or duplicate field on line ${index + 1}`);
    }
    roots.add(match[1]);
    section = match[1] === 'schema' ? undefined : match[1];
    if (match[1] === 'schema') {
      if (match[2] !== '1') invalid('schema must be 1');
    } else if (match[2] !== undefined) {
      invalid(`${match[1]} must be a mapping`);
    }
  }
  if (roots.size !== 4 || Object.entries(fields).some(([name, required]) =>
    required.some((field) => !Object.hasOwn(config[name], field)))) {
    invalid('schema, llm, seat, and paths must contain every documented field');
  }

  const llm = config.llm;
  llm.base_url = stringValue(llm.base_url, 'llm.base_url');
  llm.model = stringValue(llm.model, 'llm.model');
  llm.api_key_env = stringValue(llm.api_key_env, 'llm.api_key_env');
  llm.effort = stringValue(llm.effort, 'llm.effort');
  llm.context_max = integerValue(llm.context_max, 'llm.context_max');
  if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(llm.api_key_env)) invalid('llm.api_key_env must name an environment variable');
  if (!['l', 'm', 'h', 'x'].includes(llm.effort)) invalid('llm.effort must be l, m, h, or x');
  if (llm.model && !/^[A-Za-z0-9._:/-]+$/.test(llm.model)) invalid('llm.model must be a model name without whitespace');
  if (llm.base_url) {
    let url;
    try {
      url = new URL(llm.base_url);
    } catch {
      invalid('llm.base_url must be an HTTP(S) URL without credentials, query, or fragment');
    }
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password ||
        url.search || url.hash || !llm.model) {
      invalid('llm.base_url requires a model and an HTTP(S) URL without credentials, query, or fragment');
    }
  }

  const seat = config.seat;
  seat.id = stringValue(seat.id, 'seat.id');
  seat.principal = stringValue(seat.principal, 'seat.principal');
  seat.turn_budget = integerValue(seat.turn_budget, 'seat.turn_budget');
  if (seat.id !== 'coder' || seat.principal !== 'coder' || seat.turn_budget < 1 || seat.turn_budget > 64) {
    invalid('seat must be coder with principal coder and turn_budget between 1 and 64');
  }
  const toolList = /^\[([^\[\]]*)\]$/.exec(seat.tools);
  if (!toolList) invalid('seat.tools must be an inline list of builtin tools');
  seat.tools = toolList[1].split(',').map((tool) => tool.trim());
  if (!seat.tools.length || new Set(seat.tools).size !== seat.tools.length ||
      seat.tools.some((tool) => !availableTools.includes(tool))) {
    invalid('seat.tools may only contain distinct read_file, write_file, list_dir, run_test tools');
  }
  seat.tools = Object.freeze(seat.tools);

  for (const name of fields.paths) {
    config.paths[name] = relativePath(stringValue(config.paths[name], `paths.${name}`), `paths.${name}`);
  }
  if (!config.paths.memory.endsWith('.jsonl')) invalid('paths.memory must be a .jsonl file');
  return Object.freeze({
    schema: 1,
    llm: Object.freeze(llm),
    seat: Object.freeze(seat),
    paths: Object.freeze(config.paths),
  });
}

function readConfigFile(file) {
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

export function loadConfig({ repoRoot = rosterRoot } = {}) {
  const local = path.join(repoRoot, '.roster', 'config.yml');
  let source;
  try {
    source = readConfigFile(local);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    source = readConfigFile(path.join(repoRoot, 'roster.config.example.yml'));
  }
  return parseConfig(source);
}
