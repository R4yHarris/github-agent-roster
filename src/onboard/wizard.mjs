import { execFile } from 'node:child_process';
import { promises as fs, lstatSync } from 'node:fs';
import path from 'node:path';
import { createInterface } from 'node:readline/promises';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { parseConfig, readConfigFile, validateBaseUrl, writePrivateConfig } from '../lib/config.mjs';
import { checkDoctor, formatDoctor } from '../lib/doctor.mjs';
import { ensureLocalPath, resolveProjectRoot } from '../lib/paths.mjs';
import { resolvePublishModel } from '../metrics/run.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';

const installation = fileURLToPath(new URL('../../', import.meta.url));
const execute = promisify(execFile);
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

export async function probeModels(baseUrl, {
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
    }, probeTimeoutMs);
  });
  const request = async () => {
    const response = await fetchImpl(url.href, {
      method: 'GET', headers: { Accept: 'application/json' },
      signal: controller.signal, redirect: 'error',
    });
    if (!Number.isInteger(response.status) || response.status < 200 || response.status >= 300) {
      throw new ModelProbeError(Number.isInteger(response.status)
        ? `HTTP ${response.status}` : 'invalid response');
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
      return [...new Set(payload.data.map((entry) => modelId(entry?.id, env, apiKeyEnv)))];
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

function renderConfig(base, { baseUrl, model, publish, internet, runTest, reviewer, turns, budget }) {
  const { reviewer: _legacyReview, ...preserved } = base;
  const config = {
    ...preserved,
    llm: { ...base.llm, profile: 'vllm-local', base_url: baseUrl, model,
      api_key_env: base.profiles['vllm-local'].api_key_env, api_key_optional: true, provider: 'vllm' },
    seat: { ...base.seat, turn_budget: turns, context_chars: budget },
    publish: { enabled: publish },
    tools: { ...(internet === undefined ? {} : { internet }), run_test: runTest },
    review: { required: reviewer }, loop: { turns }, context: { budget },
  };
  const scalar = (value) => Array.isArray(value) ? `[${value.join(', ')}]`
    : typeof value === 'string' ? JSON.stringify(value) : String(value);
  const mapping = (values, indent = '') => Object.entries(values).map(([name, value]) =>
    value && typeof value === 'object' && !Array.isArray(value)
      ? `${indent}${name}:\n${mapping(value, `${indent}  `)}`
      : `${indent}${name}: ${scalar(value)}\n`).join('');
  return '# Private onboarding settings. No credentials belong in this file.\n' + mapping(config);
}

async function ensureIgnored(projectRoot) {
  const ignoreFile = path.join(projectRoot, '.gitignore');
  await ensureLocalPath(ignoreFile, projectRoot);
  let source = '';
  try {
    const entry = await fs.lstat(ignoreFile);
    if (!entry.isFile() || entry.isSymbolicLink()) throw new Error('.gitignore must be a regular file');
    source = await fs.readFile(ignoreFile, 'utf8');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  let ignored = false;
  if (lstatSync(path.join(projectRoot, '.git'), { throwIfNoEntry: false })) {
    const git = (args) => execute('git', ['--no-pager', ...args], {
      cwd: projectRoot, encoding: 'utf8', timeout: 10_000,
    });
    const { stdout } = await git(['ls-files', '--', '.roster/config.yml', '.roster/config.yml.*']);
    if (stdout.trim()) throw new Error('Private config is tracked by Git; untrack it before onboarding');
    try {
      await Promise.all(['.roster/config.yml', '.roster/config.yml.tmp'].map((file) =>
        git(['check-ignore', '--quiet', '--', file])));
      ignored = true;
    } catch (error) {
      if (error.code !== 1) throw new Error('Could not verify private config ignore rules', { cause: error });
    }
  } else {
    const lines = source.replace(/\r\n/g, '\n').split('\n');
    ignored = lines.includes('.roster/config.yml') && lines.includes('.roster/config.yml.*');
  }
  if (!ignored) {
    const separator = source && !source.endsWith('\n') ? '\n' : '';
    await fs.appendFile(ignoreFile, `${separator}.roster/config.yml\n.roster/config.yml.*\n`, {
      encoding: 'utf8', mode: 0o600,
    });
  }
}

export async function runOnboard({
  cwd = process.cwd(), installationRoot = installation,
  input = process.stdin, output = process.stdout, errorOutput = process.stderr,
  platform = process.platform, env = process.env, fetchImpl = globalThis.fetch,
  question, doctor = checkDoctor,
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
  const apiKeyEnv = base.profiles['vllm-local'].api_key_env;
  const controller = new AbortController();
  const terminal = question ? null : createInterface({ input, output, terminal: true });
  terminal?.on('SIGINT', () => controller.abort());
  terminal?.on('close', () => controller.abort());
  const ask = question ?? ((prompt) => terminal.question(prompt, { signal: controller.signal }));
  const answer = async (prompt) => {
    const value = await ask(prompt);
    if (typeof value !== 'string') throw new Error('Onboarding ended before all questions were answered');
    return value.trim();
  };
  const yesNo = async (prompt, fallback) => {
    for (;;) {
      const value = (await answer(`${prompt}? [${fallback ? 'yes' : 'no'}] `)).toLowerCase();
      if (!value) return fallback;
      if (['yes', 'y'].includes(value)) return true;
      if (['no', 'n'].includes(value)) return false;
      output.write('Please answer yes or no.\n');
    }
  };
  const integer = async (prompt, fallback, maximum = Number.MAX_SAFE_INTEGER) => {
    for (;;) {
      const value = await answer(`${prompt} [${fallback}]: `) || String(fallback);
      if (/^[1-9]\d*$/.test(value) && Number.isSafeInteger(Number(value)) && Number(value) <= maximum) {
        return Number(value);
      }
      output.write(maximum === Number.MAX_SAFE_INTEGER
        ? 'Enter a positive safe integer.\n' : `Enter an integer from 1 to ${maximum}.\n`);
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
    let baseUrl;
    for (;;) {
      const value = await answer(`vLLM base URL [${defaultBaseUrl}]: `) || defaultBaseUrl;
      try {
        baseUrl = validateBaseUrl(publicSetting(value, env, apiKeyEnv));
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
    try {
      models = await probeModels(baseUrl, { fetchImpl, env, apiKeyEnv });
    } catch (error) {
      if (!(error instanceof ModelProbeError)) throw error;
      output.write(`Model probe failed: ${error.message}\n`);
    }
    let model;
    if (models) {
      output.write('Available models:\n' +
        models.map((id, index) => `  ${index + 1}. ${id}\n`).join(''));
      for (;;) {
        const choice = await answer('Select a model [1]: ') || '1';
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
    const next = renderConfig(base, { baseUrl, model, publish, internet, runTest, reviewer, turns, budget });
    parseConfig(next);
    output.write(`\n4. Review\nConfig: ${configPath}\n` +
      `Endpoint: ${baseUrl}\nModel: ${model}\nModels probe: ${models ? 'succeeded' : 'failed; model supplied manually'}\n` +
      `Publish enabled: ${publish}\nrun_test allowed: ${runTest}\nReviewer required: ${reviewer}\n` +
      `Internet preference: ${internet === undefined ? 'not set' : internet} (stored only; no live internet tool)\n` +
      `Max tool turns: ${turns}\nContext char budget: ${budget}\n` +
      'GitHub App publication still requires GITHUB_APP_ID, a private key, and human-owned policy.\n');
    if (!await yesNo('Confirm write .roster/config.yml', true)) {
      output.write('Private config was not changed.\n');
      return { exitCode: 0, saved: false };
    }
    confirmed = true;
    await ensureIgnored(projectRoot);
    const config = await writePrivateConfig(next, { repoRoot: projectRoot, expectedSource: previous });
    output.write(`\nSaved private config: ${configPath}\n\n5. Doctor\n`);
    const checked = doctor({ cwd: projectRoot, installationRoot, env });
    output.write(formatDoctor(checked));
    if (!checked.ok) output.write('Doctor found missing prerequisites; config is saved, not a grant to publish.\n');
    return { exitCode: 0, saved: true, configPath, config, doctor: checked,
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
