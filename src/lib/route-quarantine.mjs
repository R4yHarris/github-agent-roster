import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { IDENTIFIER } from './learn.mjs';
import { ensureLocalPath } from './paths.mjs';

// A gateway that served another model for a locked request will keep doing so; remember it across runs.
export const routeQuarantineTtlMs = 24 * 60 * 60 * 1000;
const modelName = /^[A-Za-z0-9][A-Za-z0-9._:/@+-]{0,127}$/;
const maxEntries = 64;

export function routeQuarantinePath(repoRoot) {
  return path.join(repoRoot, '.roster', 'runs', 'route-quarantine.json');
}

function validEntry(entry) {
  return entry && typeof entry === 'object' && !Array.isArray(entry) &&
    typeof entry.profile === 'string' && IDENTIFIER.test(entry.profile) &&
    typeof entry.requested === 'string' && modelName.test(entry.requested) &&
    typeof entry.served === 'string' && modelName.test(entry.served) &&
    typeof entry.at === 'string' && Number.isFinite(Date.parse(entry.at));
}

async function readEntries(repoRoot) {
  const file = routeQuarantinePath(repoRoot);
  let text;
  try {
    await ensureLocalPath(file, repoRoot);
    text = await fs.readFile(file, 'utf8');
  } catch {
    return [];
  }
  // The quarantine is a routing hint, not a security boundary: an unreadable or malformed file is ignored.
  try {
    const parsed = JSON.parse(text);
    return Array.isArray(parsed?.entries) ? parsed.entries.filter(validEntry) : [];
  } catch {
    return [];
  }
}

export async function loadRouteQuarantine({ repoRoot, now = () => new Date() }) {
  const cutoff = now().getTime() - routeQuarantineTtlMs;
  const active = new Map();
  for (const entry of await readEntries(repoRoot)) {
    if (Date.parse(entry.at) >= cutoff) active.set(entry.profile, entry);
  }
  return [...active.values()];
}

export async function recordRouteQuarantine({ repoRoot, profile, requested, served, now = () => new Date() }) {
  const entry = { profile, requested, served, at: now().toISOString() };
  if (!validEntry(entry)) throw new TypeError('Route quarantine entries need a fleet profile id and safe model names');
  const cutoff = now().getTime() - routeQuarantineTtlMs;
  const entries = [...(await readEntries(repoRoot))
    .filter((existing) => existing.profile !== profile && Date.parse(existing.at) >= cutoff), entry].slice(-maxEntries);
  const file = routeQuarantinePath(repoRoot);
  await ensureLocalPath(file, repoRoot);
  await fs.mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
  await ensureLocalPath(file, repoRoot);
  const temporary = `${file}.${process.pid}.tmp`;
  const handle = await fs.open(temporary, constants.O_WRONLY | constants.O_CREAT | constants.O_TRUNC |
    (constants.O_NOFOLLOW ?? 0), 0o600);
  try {
    await handle.writeFile(`${JSON.stringify({ entries }, null, 2)}\n`, 'utf8');
  } finally {
    await handle.close();
  }
  await fs.rename(temporary, file);
  return entry;
}
