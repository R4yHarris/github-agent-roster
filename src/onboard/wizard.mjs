import path from 'node:path';
import { createInterface } from 'node:readline/promises';
import { fileURLToPath } from 'node:url';
import { formatConfig, parseConfig, readConfigFile, validateBaseUrl } from '../lib/config.mjs';
import { checkDoctor, formatDoctor } from '../lib/doctor.mjs';
import { formatFleet, normalizeFleetBaseUrl, parseFleet, validateFleet } from '../lib/fleet.mjs';
import { discoverEndpoints, discoveryCandidates, wslCandidates } from './discover.mjs';
import { ensurePrivateFilesIgnored, readPrivateFile, writePrivateDocuments } from '../lib/private-files.mjs';
import { ensureLocalPath, resolveProjectRoot } from '../lib/paths.mjs';
import { resolvePublishModel } from '../metrics/run.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';
import { reportedContextMax } from '../llm/window.mjs';
import { checkAppIdentity, formatAppIdentity } from './app.mjs';
import { resolveSecret } from '../lib/secrets.mjs';

const installation = fileURLToPath(new URL('../../', import.meta.url));
const defaultBaseUrl = 'http://127.0.0.1:8000/v1';
const probeTimeoutMs = 5_000;

class ModelProbeError extends Error {}

function publicSetting(value, env, apiKeyEnv = 'ROSTER_API_KEY') {
  if (typeof value !== 'string' || /[\x00-\x1f\x7f]/.test(value) ||
      redactEvidence(value, { env, apiKeyEnv }) !== value) {
    throw new TypeError('Enter a public URL or model ID, not a secret');
  }
  return value;
}

function modelId(value, env, apiKeyEnv) {
  const model = resolvePublishModel({ env: { AI_MODEL: publicSetting(value, env, apiKeyEnv) } });
  if (model === 'builtin-stub' || !/^[A-Za-z0-9][A-Za-z0-9._:/-]*$/.test(model)) {
    throw new TypeError('Model ID must use the supported served-model alphabet');
  }
  return model;
}

export async function probeModelDetails(baseUrl, {
  fetchImpl = globalThis.fetch, env = process.env, schedule = setTimeout, cancel = clearTimeout,
  apiKeyEnv = 'ROSTER_API_KEY',
} = {}) {
  const url = new URL(validateBaseUrl(publicSetting(baseUrl, env, apiKeyEnv)));
  url.pathname = `${url.pathname.replace(/\/+$/, '')}/models`;
  const controller = new AbortController();
  let timer;
  const deadline = new Promise((_, reject) => {
    timer = schedule(() => {
      controller.abort();
      reject(new ModelProbeError('timeout'));
    }, url.hostname === '127.0.0.1' || url.hostname === 'localhost' ? probeTimeoutMs : 60_000);
  });
  const request = async () => {
    const aperture = url.hostname.includes('aperture');
    const names = aperture ? ['APERTURE_API_KEY', 'ROSTER_API_KEY'] : [apiKeyEnv];
    let source = 'missing';
    let key;
    for (const name of names) {
      try {
        key = await resolveSecret(name, { env });
      } catch {
        key = undefined;
      }
      if (key) {
        source = env[name] ? 'env' : 'vault';
        break;
      }
    }
    const headers = { Accept: 'application/json', ...(key ? { Authorization: `Bearer ${key}` } : {}) };
    const response = await fetchImpl(url.href, {
      method: 'GET', headers, signal: controller.signal, redirect: 'error',
    });
    if (!Number.isInteger(response.status) || response.status < 200 || response.status >= 300) {
      throw new ModelProbeError(Number.isInteger(response.status)
        ? `HTTP ${response.status} auth=${source}` : 'invalid response');
    }
    let payload;
    try {
      payload = await response.json();
    } catch (error) {
      if (!(error instanceof Error)) throw error;
      throw new ModelProbeError('invalid response');
    }
    if (!Array.isArray(payload?.data) || !payload.data.length) {
      throw new ModelProbeError('invalid response');
    }
    try {
      const models = new Map();
      for (const entry of payload.data) {
        const id = modelId(entry?.id, env, apiKeyEnv);
        const contextMax = reportedContextMax(entry);
        if (!models.has(id) || models.get(id).context_max === undefined && contextMax !== undefined) {
          models.set(id, Object.freeze({ id, ...(contextMax === undefined ? {} : { context_max: contextMax }) }));
        }
      }
      return [...models.values()];
    } catch (error) {
      if (!(error instanceof Error)) throw error;
      throw new ModelProbeError('invalid response');
    }
  };
  try {
    return await Promise.race([request(), deadline]);
  } catch (error) {
    if (error instanceof ModelProbeError) throw error;
    if (!(error instanceof Error)) throw error;
    throw new ModelProbeError(error.code === 'ECONNREFUSED' || error.cause?.code === 'ECONNREFUSED'
      ? 'refused' : 'network');
  } finally {
    cancel(timer);
    controller.abort();
  }
}

export async function probeModels(baseUrl, options) {
  return (await probeModelDetails(baseUrl, options)).map(({ id }) => id);
}

function renderConfig(base, { baseUrl, model, contextMax, publish, internet, runTest, reviewer, turns, budget }) {
  const { reviewer: _legacyReview, ...preserved } = base;
  const config = {
    ...preserved,
    llm: { ...base.llm, profile: 'vllm-local', base_url: baseUrl, model, context_max: contextMax,
      api_key_env: base.profiles['vllm-local'].api_key_env, api_key_optional: true, provider: 'vllm' },
    seat: { ...base.seat, turn_budget: turns, context_chars: budget },
    publish: { enabled: publish },
    tools: { ...(internet === undefined ? {} : { internet }), run_test: runTest },
    review: { required: reviewer }, loop: { turns }, context: { budget },
  };
  return formatConfig(config);
}

export async function runOnboard({
  cwd = process.cwd(), installationRoot = installation,
  input = process.stdin, output = process.stdout, errorOutput = process.stderr,
  platform = process.platform, env = process.env, fetchImpl = globalThis.fetch,
  question, doctor = checkDoctor, discover = false,
} = {}) {
  if (!input.isTTY || !output.isTTY) {
    errorOutput.write('roster onboard needs a terminal\n');
    return { exitCode: 2, saved: false };
  }
  if (!['win32', 'linux', 'darwin'].includes(platform)) {
    throw new Error('roster onboard supports win32, linux, and darwin');
  }
  output.write(`Roster onboarding\n\n1. Platform\nOS: ${platform}\n`);
  const projectRoot = resolveProjectRoot(cwd);
  const configPath = path.join(projectRoot, '.roster', 'config.yml');
  const fleetPath = path.join(projectRoot, '.roster', 'fleet.yml');
  await ensureLocalPath(configPath, projectRoot);
  let previous = null;
  try {
    previous = readConfigFile(configPath);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  let source = previous;
  if (source === null) {
    try {
      source = readConfigFile(path.join(projectRoot, 'roster.config.example.yml'));
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
      source = readConfigFile(path.join(installationRoot, 'roster.config.example.yml'));
    }
  }
  const base = parseConfig(source);
  const previousFleet = await readPrivateFile(projectRoot, 'fleet.yml');
  const fleet = previousFleet === null ? validateFleet({ profiles: [] }) : parseFleet(previousFleet);
  const apiKeyEnv = base.profiles['vllm-local'].api_key_env;
  const controller = new AbortController();
  const terminal = question ? null : createInterface({ input, output, terminal: true });
  terminal?.on('SIGINT', () => controller.abort());
  terminal?.on('close', () => controller.abort());
  const ask = question ?? ((prompt) => terminal.question(prompt, { signal: controller.signal }));
  const answer = async (prompt, fallback = '') => {
    const value = await ask(prompt);
    if (typeof value !== 'string') throw new Error('Onboarding ended before all questions were answered');
    return value.trim() || fallback;
  };
  const yesNo = async (prompt, fallback) => {
    const defaultAnswer = fallback ? 'yes' : 'no';
    for (;;) {
      const value = (await answer(`${prompt}? [${defaultAnswer}] `, defaultAnswer)).toLowerCase();
      if (['yes', 'y'].includes(value)) return true;
      if (['no', 'n'].includes(value)) return false;
      output.write('Please answer yes or no.\n');
    }
  };
  const integer = async (prompt, fallback, maximum = Number.MAX_SAFE_INTEGER, minimum = 1) => {
    for (;;) {
      const value = await answer(`${prompt} [${fallback}]: `, String(fallback));
      if (/^(?:0|[1-9]\d*)$/.test(value) && Number.isSafeInteger(Number(value)) &&
          Number(value) >= minimum && Number(value) <= maximum) {
        return Number(value);
      }
      output.write(maximum === Number.MAX_SAFE_INTEGER
        ? `Enter a ${minimum === 0 ? 'nonnegative' : 'positive'} safe integer.\n`
        : `Enter an integer from ${minimum} to ${maximum}.\n`);
    }
  };
  let confirmed = false;
  try {
    if (previous !== null && !await yesNo('Replace onboarding settings in the existing private config', false)) {
      output.write('Kept existing private config unchanged.\n');
      return { exitCode: 0, saved: false };
    }
    output.write('\n2. LLM endpoint\n');
    output.write('WSL talking to a Windows-hosted server may need the Windows host IP, not localhost.\n');
    let discoveredDefault = defaultBaseUrl;
    if (discover) {
      output.write('Scanning local, LAN, and ROSTER_DISCOVER_HOSTS endpoints.\n');
      let extraUrls = [];
      if (platform === 'win32' && await yesNo('Allow a read-only model probe inside WSL', false)) {
        try {
          extraUrls = await wslCandidates();
          output.write(extraUrls.length
            ? `WSL answered on ${extraUrls.join(', ')}.\n`
            : 'WSL probe found no local OpenAI-compatible server.\n');
        } catch (error) {
          output.write(`WSL probe skipped: ${error instanceof Error ? error.message : 'unavailable'}.\n`);
        }
      }
      const found = await discoverEndpoints({ env, fetchImpl, candidates: discoveryCandidates(env, extraUrls) });
      if (!found.length) output.write('No OpenAI-compatible server answered. Enter a base URL manually.\n');
      found.forEach((entry, index) => {
        output.write(`  ${index + 1}. ${entry.baseUrl} models=${entry.models.map(({ id, context_max: limit }) =>
          `${id}${limit ? ` ctx=${limit}` : ' ctx=unreported'}`).join(', ')}\n`);
      });
      if (found.length) {
        output.write('Name the hardware for each server. Detection cannot see the GPU.\n');
        for (const entry of found) {
          const fallback = entry.baseUrl.includes(':8888') ? 'DGX Spark'
            : entry.baseUrl.includes('aperture') ? 'RTX 6000'
              : entry.baseUrl.includes(':11435') ? 'RTX 3090' : 'unspecified';
          entry.hardware = await answer(`Hardware for ${entry.baseUrl} [${fallback}]: `, fallback);
        }
      }
      const preferred = found.find((entry) => entry.baseUrl.includes(':8888')) ?? found[0];
      if (found.length && await yesNo(`Use ${preferred.baseUrl} as the default`, false)) {
        discoveredDefault = preferred.baseUrl;
      }
    }
    let baseUrl;
    for (;;) {
      const value = await answer(`vLLM base URL [${discoveredDefault}]: `, discoveredDefault);
      try {
        baseUrl = normalizeFleetBaseUrl(publicSetting(value, env, apiKeyEnv));
        if (/\/(?:models|chat\/completions)\/?$/.test(new URL(baseUrl).pathname)) {
          throw new TypeError('Use a base URL, not a complete models or chat endpoint');
        }
        break;
      } catch (error) {
        if (!(error instanceof Error)) throw error;
        output.write('Enter an HTTP(S) base URL without credentials, a query, or a fragment; not a full /models or /chat/completions endpoint.\n');
      }
    }
    let models;
    let modelDetails;
    try {
      modelDetails = await probeModelDetails(baseUrl, { fetchImpl, env, apiKeyEnv });
      models = modelDetails.map(({ id }) => id);
    } catch (error) {
      if (!(error instanceof ModelProbeError)) throw error;
      output.write(`Model probe failed: ${error.message}\n`);
    }
    let model;
    if (models) {
      output.write('Available models:\n' +
        models.map((id, index) => `  ${index + 1}. ${id}\n`).join(''));
      for (;;) {
        const choice = await answer('Select a model [1]: ', '1');
        if (/^[1-9]\d*$/.test(choice) && Number.isSafeInteger(Number(choice)) && Number(choice) <= models.length) {
          model = models[Number(choice) - 1];
          break;
        }
        output.write('Enter a listed model number.\n');
      }
    } else {
      for (;;) {
        try {
          model = modelId(await answer('Model ID [required]: '), env, apiKeyEnv);
          break;
        } catch (error) {
          if (!(error instanceof TypeError)) throw error;
          output.write('Enter the actual single-line served model ID, not unknown or a secret.\n');
        }
      }
    }
    const reportedContext = modelDetails?.find(({ id }) => id === model)?.context_max;
    const contextMax = reportedContext ?? await integer('Model context tokens (0 = unknown)', base.llm.context_max,
      Number.MAX_SAFE_INTEGER, 0);
    if (reportedContext !== undefined) {
      output.write(`Using context_max=${contextMax} reported by /v1/models for ${model}.\n`);
    }
    if (!await yesNo('Add more endpoints later with roster fleet add. Continue', true)) {
      output.write('Private config and fleet were not changed.\n');
      return { exitCode: 0, saved: false };
    }
    output.write('\n3. Permissions (local preferences, not GitHub policy grants)\n');
    output.write('These flags do not grant contracts policy.\n');
    const publish = await yesNo('Allow publish through GitHub App', true);
    const reviewer = await yesNo('Require reviewer before publish', true);
    const runTest = await yesNo('Allow run_test', true);
    const advanced = await yesNo('Show advanced settings', false);
    let internet;
    let turns = base.loop?.turns ?? 12;
    let budget = base.context?.budget ?? base.seat.context_chars;
    if (advanced) {
      output.write('\nAdvanced\n');
      internet = await yesNo('Internet search and research outside the worktree (stored only)', true);
      turns = await integer('Max tool turns', 12, 64);
      budget = await integer('Context char budget', 8000);
    }
    const next = renderConfig(base, { baseUrl, model, contextMax, publish, internet, runTest, reviewer, turns, budget });
    const config = parseConfig(next);
    const seeded = validateFleet({ profiles: [{
      id: 'default', base_url: baseUrl, model, provider: 'vllm', context_max: contextMax,
      concurrency: 1, hardware: 'unspecified', notes: 'Default endpoint selected during onboarding.',
    }, ...fleet.profiles.filter(({ id }) => id !== 'default')] });
    const appStatus = publish ? await checkAppIdentity({ cwd: projectRoot, env }) : null;
    if (appStatus) output.write(`\nApp identity (environment only)\n${formatAppIdentity(appStatus)}`);
    output.write(`\n4. Review\nConfig: ${configPath}\nFleet: ${fleetPath} (default profile; other profiles kept)\n` +
      `Endpoint: ${baseUrl}\nModel: ${model}\nModels probe: ${models ? 'succeeded' : 'failed; model supplied manually'}\n` +
      `Model context tokens: ${contextMax || 'unknown'}\n` +
      `Publish enabled: ${publish}\nrun_test allowed: ${runTest}\nReviewer required: ${reviewer}\n` +
      `Internet preference: ${internet === undefined ? 'not set' : internet} (stored only; no live internet tool)\n` +
      `Max tool turns: ${turns}\nContext char budget: ${budget}\n` +
      'GitHub App publication still requires GITHUB_APP_ID, a private key, and human-owned policy.\n');
    if (!await yesNo('Confirm write .roster/config.yml', true)) {
      output.write('Private config was not changed.\n');
      return { exitCode: 0, saved: false };
    }
    confirmed = true;
    await ensurePrivateFilesIgnored(projectRoot, ['config.yml', 'fleet.yml']);
    await writePrivateDocuments([
      { name: 'fleet.yml', source: formatFleet(seeded), expectedSource: previousFleet },
      { name: 'config.yml', source: next, expectedSource: previous },
    ], { repoRoot: projectRoot });
    output.write(`\nSaved private config: ${configPath}\nSaved fleet: ${fleetPath}\n\n5. Doctor\n`);
    const checked = doctor({ cwd: projectRoot, installationRoot, env });
    output.write(formatDoctor(checked));
    if (!checked.ok) output.write('Doctor found missing prerequisites; config is saved, not a grant to publish.\n');
    return { exitCode: checked.ok ? 0 : 1, saved: true, configPath, fleetPath, config, fleet: seeded,
      doctor: checked, appStatus,
      modelsProbed: Boolean(models) };
  } catch (error) {
    if (error instanceof Error && error.name === 'AbortError') {
      output.write('\nOnboarding cancelled; private config was not changed.\n');
      return { exitCode: confirmed ? 130 : 0, saved: false };
    }
    throw error;
  } finally {
    terminal?.close();
  }
}
