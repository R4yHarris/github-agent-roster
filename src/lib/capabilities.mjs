import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { formatCatalogYaml, parseCatalogYaml } from './catalog-yaml.mjs';
import { readConfigFile } from './config.mjs';
import { validateFleetId } from './fleet.mjs';
import { ensureLocalPath, resolveProjectRoot } from './paths.mjs';
import { readPrivateFile } from './private-files.mjs';
import { resolvePublishModel } from '../metrics/run.mjs';

const installation = fileURLToPath(new URL('../../', import.meta.url));
const fields = ['profile_id', 'model_id', 'task_class', 'suggested_difficulty', 'context_max', 'concurrency', 'notes'];
const required = ['suggested_difficulty', 'context_max', 'concurrency', 'notes'];
const taskClasses = ['feat', 'fix', 'docs', 'test'];
const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

function key(record) {
  return JSON.stringify([record.profile_id === undefined ? 'model' : 'profile',
    record.profile_id ?? record.model_id, record.task_class]);
}

function validateRecord(record, partial) {
  if (!isObject(record) || Object.keys(record).some((field) => !fields.includes(field)) ||
      Object.hasOwn(record, 'profile_id') === Object.hasOwn(record, 'model_id') ||
      !taskClasses.includes(record.task_class) ||
      (!partial && required.some((field) => !Object.hasOwn(record, field)))) {
    throw new TypeError('Capability prior needs one profile_id or model_id, a task_class, and documented fields');
  }
  if (Object.hasOwn(record, 'profile_id')) validateFleetId(record.profile_id);
  else {
    const model = resolvePublishModel({ env: { AI_MODEL: record.model_id } });
    if (model === 'builtin-stub') throw new TypeError('Capability model_id must name an actual model');
  }
  if (Object.hasOwn(record, 'suggested_difficulty') &&
      (!Number.isInteger(record.suggested_difficulty) || record.suggested_difficulty < 1 ||
        record.suggested_difficulty > 5)) {
    throw new TypeError('Capability suggested_difficulty must be an integer from 1 to 5');
  }
  for (const field of ['context_max', 'concurrency']) {
    if (Object.hasOwn(record, field) && (!Number.isSafeInteger(record[field]) || record[field] < 1)) {
      throw new TypeError(`Capability ${field} must be a positive safe integer`);
    }
  }
  if (Object.hasOwn(record, 'notes') && (typeof record.notes !== 'string' ||
      !record.notes.trim() || record.notes !== record.notes.trim() || record.notes.length > 240 ||
      /[\x00-\x1f\x7f]/.test(record.notes) ||
      /(?:\d{1,3}\.){3}\d{1,3}|(?:[a-f0-9]{0,4}:){2,}[a-f0-9]{0,4}/i.test(record.notes))) {
    throw new TypeError('Capability notes must be short single-line text without IP addresses');
  }
  return Object.freeze({ ...record });
}

export function validateCapabilities(catalog, { partial = false } = {}) {
  if (!isObject(catalog) || Object.keys(catalog).length !== 1 || !Array.isArray(catalog.capabilities)) {
    throw new TypeError('Capabilities must contain only a capabilities list');
  }
  const capabilities = catalog.capabilities.map((record) => validateRecord(record, partial));
  if (new Set(capabilities.map(key)).size !== capabilities.length) {
    throw new TypeError('Capability selector and task_class pairs must be unique');
  }
  return Object.freeze({ capabilities: Object.freeze(capabilities) });
}

export function parseCapabilities(source, options) {
  return validateCapabilities({ capabilities: parseCatalogYaml(source, { root: 'capabilities', fields }) }, options);
}

export function formatCapabilities(catalog, options) {
  return formatCatalogYaml('capabilities', validateCapabilities(catalog, options).capabilities);
}

export async function loadCapabilities({
  cwd = process.cwd(), repoRoot = resolveProjectRoot(cwd), installationRoot = installation,
} = {}) {
  const file = path.join(installationRoot, 'examples', 'capabilities.yml');
  await ensureLocalPath(file, installationRoot);
  const example = parseCapabilities(readConfigFile(file));
  const source = await readPrivateFile(repoRoot, 'capabilities.yml');
  if (source === null) return example;
  const overlay = parseCapabilities(source, { partial: true });
  const merged = new Map(example.capabilities.map((record) => [key(record), record]));
  for (const record of overlay.capabilities) {
    merged.set(key(record), validateRecord({ ...merged.get(key(record)), ...record }, false));
  }
  return validateCapabilities({ capabilities: [...merged.values()] });
}
