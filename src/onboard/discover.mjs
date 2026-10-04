import { execFile } from 'node:child_process';
import os from 'node:os';
import { promisify } from 'node:util';
import { probeModelDetails } from './wizard.mjs';

const execFileAsync = promisify(execFile);
const ports = [8000, 8001, 8888, 11434, 11435, 1234];

export function localCandidates() {
  return ports.map((port) => `http://127.0.0.1:${port}/v1`);
}

export function lanCandidates() {
  const urls = [];
  for (const entries of Object.values(os.networkInterfaces())) {
    for (const entry of entries ?? []) {
      if (!entry || entry.internal || entry.family !== 'IPv4') continue;
      const [a, b, c] = entry.address.split('.');
      if (!['10', '192', '172'].includes(a)) continue;
      for (const host of [1, 48, 254]) {
        for (const port of [8000, 8888]) urls.push(`http://${a}.${b}.${c}.${host}:${port}/v1`);
      }
    }
  }
  return urls;
}

export function discoveryCandidates(env = process.env, extraUrls = []) {
  const extra = String(env.ROSTER_DISCOVER_HOSTS ?? '').split(',').map((value) => value.trim()).filter(Boolean);
  return [...new Set([...localCandidates(), ...lanCandidates(), ...extra, ...extraUrls])];
}

async function ollamaContext(baseUrl, model, fetchImpl) {
  const url = new URL(baseUrl);
  url.pathname = '/api/show';
  url.search = '';
  const response = await fetchImpl(url.href, {
    method: 'POST', headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    body: JSON.stringify({ model }), redirect: 'error',
  });
  if (!response.ok) return undefined;
  const payload = await response.json();
  const values = Object.values(payload?.model_info ?? {}).filter((value) =>
    Number.isSafeInteger(value) && value >= 1024);
  const match = String(payload?.parameters ?? '').match(/num_ctx\s+(\d+)/);
  return values[0] ?? (match ? Number(match[1]) : undefined);
}

export async function wslCandidates() {
  const script = ports.map((port) =>
    `curl -fsS -m 2 http://127.0.0.1:${port}/v1/models >/dev/null && echo http://127.0.0.1:${port}/v1`).join('; ');
  const { stdout } = await execFileAsync('wsl.exe', ['-e', 'sh', '-lc', script], { timeout: 20_000, windowsHide: true });
  return stdout.split(/\r?\n/).map((line) => line.trim()).filter((line) => line.startsWith('http://'));
}

export async function discoverEndpoints({
  env = process.env, fetchImpl = globalThis.fetch, candidates = discoveryCandidates(env),
} = {}) {
  const found = [];
  await Promise.all(candidates.map(async (baseUrl) => {
    try {
      const models = await probeModelDetails(baseUrl, { fetchImpl, env });
      const enriched = [];
      for (const model of models) {
        let context = model.context_max;
        if (!context && /:1143[45]\//.test(baseUrl)) {
          context = await ollamaContext(baseUrl, model.id, fetchImpl).catch(() => undefined);
        }
        enriched.push({ ...model, ...(context ? { context_max: context } : {}) });
      }
      found.push({ baseUrl, models: enriched, local: baseUrl.startsWith('http://') });
    } catch {
      // A closed port is not a configuration error.
    }
  }));
  return found.sort((left, right) => left.baseUrl.localeCompare(right.baseUrl));
}
