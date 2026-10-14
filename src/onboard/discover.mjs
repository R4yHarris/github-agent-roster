import { execFile } from 'node:child_process';
import os from 'node:os';
import { promisify } from 'node:util';
import { isIP } from 'node:net';
import { probeModelDetails } from './wizard.mjs';

const execFileAsync = promisify(execFile);
const ports = [8000, 8001, 8080, 8888, 11434, 11435, 1234];

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
  const response = await fetchImpl(url.href, {
    method: 'POST', headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
    body: JSON.stringify({ model }), signal: AbortSignal.timeout(5_000), redirect: 'error',
  });
  if (!response.ok) throw new Error(`Ollama metadata HTTP ${response.status}`);
  const payload = await response.json();
  const match = typeof payload.parameters === 'string' && payload.parameters.match(/^num_ctx\s+(\d+)\s*$/m);
  const context = match ? Number(match[1]) : undefined;
  return Number.isSafeInteger(context) && context > 0 ? context : undefined;
}

export async function wslCandidates({ run = execFileAsync } = {}) {
  const { stdout } = await run('wsl.exe', ['--list', '--running', '--quiet'],
    { timeout: 20_000, windowsHide: true, encoding: 'utf16le' });
  const distributions = stdout.replace(/\0/g, '').split(/\r?\n/)
    .map((line) => line.trim()).filter((line) => line && !line.startsWith('docker-desktop'));
  if (distributions.length > 8) throw new Error('WSL discovery supports at most eight running distributions');
  const urls = new Set();
  for (const distribution of distributions) {
    const result = await run('wsl.exe', ['--distribution', distribution, '--exec', 'hostname', '-I'],
      { timeout: 5_000, windowsHide: true, encoding: 'utf8' });
    for (const ip of result.stdout.trim().split(/\s+/).filter((value) => isIP(value) === 4)) {
      for (const port of ports) urls.add(`http://${ip}:${port}/v1`);
    }
  }
  return [...urls];
}

export async function discoverEndpoints({
  env = process.env, fetchImpl = globalThis.fetch, candidates = discoveryCandidates(env),
  onFailure = () => {},
} = {}) {
  const found = [];
  await Promise.all([...new Set(candidates)].map(async (baseUrl) => {
    try {
      const models = await probeModelDetails(baseUrl, { fetchImpl, env, timeoutMs: 5_000, authenticate: false });
      if (/^1143[45]$/.test(new URL(baseUrl).port)) {
        await Promise.all(models.map(async (model, index) => {
          if (model.context_max !== undefined) return;
          try {
            const context = await ollamaContext(baseUrl, model.id, fetchImpl);
            if (context !== undefined) models[index] = { ...model, context_max: context };
          } catch {
            onFailure({ baseUrl, reason: 'Ollama context metadata unavailable; capacity remains unknown' });
          }
        }));
      }
      found.push({ baseUrl, models });
    } catch (error) {
      onFailure({ baseUrl, reason: error instanceof Error ? error.message : 'unavailable' });
    }
  }));
  return found.sort((left, right) => left.baseUrl.localeCompare(right.baseUrl));
}
