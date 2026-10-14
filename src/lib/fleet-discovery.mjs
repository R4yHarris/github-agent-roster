import { createHash } from 'node:crypto';
import { normalizeFleetBaseUrl, validateFleet } from './fleet.mjs';
import { discoveryCandidates, discoverEndpoints, wslCandidates } from '../onboard/discover.mjs';

export function mergeDiscoveredProfiles(fleet, endpoints) {
  const profiles = [...validateFleet(fleet).profiles];
  for (const endpoint of endpoints) {
    const baseUrl = normalizeFleetBaseUrl(endpoint.baseUrl);
    for (const model of endpoint.models) {
      if (profiles.some((profile) => profile.base_url === baseUrl && profile.model === model.id)) continue;
      if (!Number.isSafeInteger(model.context_max) || model.context_max < 1) continue;
      const digest = createHash('sha256').update(`${baseUrl}\n${model.id}`).digest('hex').slice(0, 16);
      const prefix = `discovered-${digest}`;
      let id = prefix;
      for (let suffix = 2; profiles.some((profile) => profile.id === id); suffix++) id = `${prefix}-${suffix}`;
      profiles.push({
        id, base_url: baseUrl, model: model.id, provider: 'vllm',
        context_max: model.context_max, concurrency: 1,
        hardware: endpoint.hardware ?? 'unspecified', notes: 'Observed through GET /models; hardware and capacity not inferred.',
      });
    }
  }
  return validateFleet({ profiles });
}

export async function discoverFleet(fleet, {
  env = process.env, fetchImpl = globalThis.fetch, wsl = false,
  discover = discoverEndpoints, discoverWsl = wslCandidates, onFailure = () => {},
} = {}) {
  let extra = [];
  if (wsl) extra = await discoverWsl();
  const endpoints = await discover({
    env, fetchImpl, onFailure,
    candidates: discoveryCandidates(env, [...extra, ...fleet.profiles.map(({ base_url }) => base_url)]),
  });
  return { endpoints, fleet: mergeDiscoveredProfiles(fleet, endpoints) };
}
