import path from 'node:path';
import { parseCatalogYaml, formatCatalogYaml } from './catalog-yaml.mjs';
import { readConfigFile, validateBaseUrl } from './config.mjs';
import { ensureLocalPath, resolveProjectRoot } from './paths.mjs';
import { ensurePrivateFilesIgnored, writePrivateDocuments } from './private-files.mjs';
import { resolvePublishModel } from '../metrics/run.mjs';

const fields = ['id', 'base_url', 'model', 'provider', 'context_max', 'concurrency',
  'hardware', 'task_class', 'notes'];
const required = fields.filter((field) => field !== 'task_class');
const taskClasses = ['feat', 'fix', 'docs', 'test'];
const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);

function text(value, label, { empty = false, maximum = 500 } = {}) {
  if (typeof value !== 'string' || (!empty && !value.trim()) ||
      value !== value.trim() || value.length > maximum || /[\x00-\x1f\x7f]/.test(value)) {
    throw new TypeError(`Fleet ${label} must be ${empty ? '' : 'nonempty '}single-line text`);
  }
  return value;
}

export function normalizeFleetBaseUrl(value) {
  text(value, 'base_url', { maximum: 2048 });
  const url = new URL(validateBaseUrl(value, 'fleet base_url'));
  const pathname = url.pathname.replace(/\/+$/, '');
  if (/\/(?:models|chat\/completions)$/.test(pathname)) {
    throw new TypeError('Fleet base_url must be an API base, not a complete request endpoint');
  }
  url.pathname = pathname.endsWith('/v1') ? pathname : `${pathname}/v1`;
  return url.href;
}

export function validateFleetProfile(profile) {
  if (!isObject(profile) || required.some((field) => !Object.hasOwn(profile, field)) ||
      Object.keys(profile).some((field) => !fields.includes(field))) {
    throw new TypeError('Fleet profile must contain the documented fields');
  }
  const id = text(profile.id, 'id', { maximum: 64 });
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(id)) {
    throw new TypeError('Fleet id must be an opaque identifier of at most 64 characters');
  }
  const model = resolvePublishModel({ env: { AI_MODEL: profile.model } });
  if (model === 'builtin-stub') throw new TypeError('Fleet model must be an actual served model');
  if (profile.provider !== 'vllm') throw new TypeError('Fleet provider must be vllm');
  if (!Number.isSafeInteger(profile.context_max) || profile.context_max < 0 ||
      (profile.context_max === 0 && id !== 'default')) {
    throw new TypeError('Fleet context_max must be positive; only the onboard default may use 0 for unknown');
  }
  if (!Number.isSafeInteger(profile.concurrency) || profile.concurrency < 1) {
    throw new TypeError('Fleet concurrency must be a positive safe integer');
  }
  if (profile.task_class !== undefined && (!Array.isArray(profile.task_class) ||
      !profile.task_class.length || new Set(profile.task_class).size !== profile.task_class.length ||
      profile.task_class.some((item) => !taskClasses.includes(item)))) {
    throw new TypeError('Fleet task_class must contain distinct feat, fix, docs, or test hints');
  }
  return Object.freeze({
    id, base_url: normalizeFleetBaseUrl(profile.base_url), model, provider: profile.provider,
    context_max: profile.context_max, concurrency: profile.concurrency,
    hardware: text(profile.hardware, 'hardware', { maximum: 120 }),
    ...(profile.task_class ? { task_class: Object.freeze([...profile.task_class]) } : {}),
    notes: text(profile.notes, 'notes', { empty: true }),
  });
}

export function validateFleet(catalog) {
  if (!isObject(catalog) || Object.keys(catalog).length !== 1 || !Array.isArray(catalog.profiles)) {
    throw new TypeError('Fleet catalog must contain only a profiles list');
  }
  const profiles = catalog.profiles.map(validateFleetProfile);
  if (new Set(profiles.map(({ id }) => id)).size !== profiles.length) {
    throw new TypeError('Fleet profile IDs must be unique');
  }
  return Object.freeze({ profiles: Object.freeze(profiles) });
}

export function parseFleet(source) {
  return validateFleet({ profiles: parseCatalogYaml(source, { root: 'profiles', fields }) });
}

export function formatFleet(catalog) {
  return formatCatalogYaml('profiles', validateFleet(catalog).profiles);
}

export function getFleetProfile(catalog, id) {
  const profile = catalog.profiles.find((item) => item.id === id);
  if (!profile) throw new Error('Fleet profile ID was not found in .roster/fleet.yml');
  return profile;
}

export async function loadFleet({ cwd = process.cwd(), repoRoot = resolveProjectRoot(cwd) } = {}) {
  const file = path.join(repoRoot, '.roster', 'fleet.yml');
  await ensureLocalPath(file, repoRoot);
  let source;
  try {
    source = readConfigFile(file);
  } catch (error) {
    if (error.code === 'ENOENT') return validateFleet({ profiles: [] });
    throw error;
  }
  return parseFleet(source);
}

export async function writeFleet(catalog, {
  cwd = process.cwd(), repoRoot = resolveProjectRoot(cwd), expectedSource,
} = {}) {
  const fleet = validateFleet(catalog);
  await ensurePrivateFilesIgnored(repoRoot, ['fleet.yml']);
  await writePrivateDocuments([{ name: 'fleet.yml', source: formatFleet(fleet), expectedSource }], { repoRoot });
  return fleet;
}
