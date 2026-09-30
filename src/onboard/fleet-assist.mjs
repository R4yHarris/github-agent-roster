import { promises as fs } from 'node:fs';
import path from 'node:path';
import { createInterface } from 'node:readline/promises';
import { fileURLToPath } from 'node:url';
import { TextDecoder } from 'node:util';
import { parseConfig } from '../lib/config.mjs';
import { formatFleet, normalizeFleetBaseUrl, parseFleet, validateFleet, validateFleetId,
  validateFleetProfile, writeFleet } from '../lib/fleet.mjs';
import { createBuiltinChat } from '../lib/llm.mjs';
import { ensureLocalPath, resolveProjectRoot } from '../lib/paths.mjs';
import { readPrivateFile } from '../lib/private-files.mjs';
import { resolvePublishModel } from '../metrics/run.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';
import { probeModelDetails } from './wizard.mjs';

const installation = fileURLToPath(new URL('../../', import.meta.url));
const fields = ['id', 'base_url', 'model', 'hardware', 'context_max', 'concurrency', 'task_class', 'notes'];
const hardRule = 'Hard rule: do not invent URLs or model IDs. Record only operator answers or actual ' +
  'GET /v1/models IDs and reported positive context limits. Ask one question for the requested field, with no URLs, suggestions, or tools. ' +
  'A final profile must exactly preserve recorded facts. You cannot write files or authorize a write.';

async function loadInterviewTemplate(installationRoot) {
  const file = path.join(installationRoot, 'templates', 'fleet', 'ASK.md');
  await ensureLocalPath(file, installationRoot);
  const entry = await fs.lstat(file);
  if (!entry.isFile() || entry.isSymbolicLink() || entry.size > 65_536) {
    throw new Error('Fleet interview template must be a regular file of at most 64 KiB');
  }
  const content = new TextDecoder('utf-8', { fatal: true })
    .decode(await fs.readFile(file)).replace(/\r\n/g, '\n');
  const questions = new Map();
  for (const line of content.split('\n')) {
    const match = /^- ([a-z_]+): (.+)$/.exec(line);
    if (!match || !fields.includes(match[1])) continue;
    if (questions.has(match[1])) throw new Error('Fleet interview template has duplicate questions');
    questions.set(match[1], match[2]);
  }
  if (questions.size !== fields.length) throw new Error('Fleet interview template must define every field question');
  return { content, questions };
}

function redact(text, config, env) {
  let safe = text;
  for (const apiKeyEnv of [config.llm.api_key_env,
    ...Object.values(config.profiles).map((profile) => profile.api_key_env)]) {
    safe = redactEvidence(safe, { env, apiKeyEnv });
  }
  return safe;
}

function safeAnswer(value, config, env) {
  if (/[\x00-\x1f\x7f]/.test(value) || redact(value, config, env) !== value) {
    throw new TypeError('Do not supply control characters, credentials, or secrets as fleet facts');
  }
  return value;
}

function parseAnswer(field, value, facts, models, config, env) {
  safeAnswer(value, config, env);
  if (field === 'id') return validateFleetId(value);
  if (field === 'base_url') return normalizeFleetBaseUrl(value);
  if (field === 'model') {
    const selected = models?.includes(value) ? value
      : models && /^[1-9]\d*$/.test(value) && Number(value) <= models.length ? models[Number(value) - 1] : value;
    if (models && !models.includes(selected)) throw new TypeError('Choose an ID returned by this endpoint models list');
    const model = resolvePublishModel({ env: { AI_MODEL: selected } });
    if (model === 'builtin-stub') throw new TypeError('Supply an actual model ID, not a stub');
    return model;
  }
  if (field === 'context_max' || field === 'concurrency') {
    const minimum = field === 'context_max' && facts.id === 'default' ? 0 : 1;
    if (!/^(?:0|[1-9]\d*)$/.test(value) || !Number.isSafeInteger(Number(value)) || Number(value) < minimum) {
      throw new TypeError(`Fleet ${field} needs a ${minimum === 0 ? 'nonnegative' : 'positive'} safe integer`);
    }
    return Number(value);
  }
  if (field === 'task_class') {
    if (!value) return undefined;
    const classes = value.split(',').map((item) => item.trim());
    if (new Set(classes).size !== classes.length || classes.some((item) => !['feat', 'fix', 'docs', 'test'].includes(item))) {
      throw new TypeError('Task-class hints must be distinct feat, fix, docs, or test values, or blank');
    }
    return classes;
  }
  if ((field === 'hardware' && !value) || value.length > (field === 'hardware' ? 120 : 500)) {
    throw new TypeError(`Fleet ${field} must be ${field === 'hardware' ? 'nonempty ' : ''}short single-line text`);
  }
  return value;
}

function responseObject(response) {
  if (response.message?.role !== 'assistant' || response.message.tool_calls !== undefined ||
      response.message.function_call !== undefined || !['stop', null, undefined].includes(response.finish_reason) ||
      typeof response.message.content !== 'string') {
    throw new Error('Default model returned an unsupported interview response or tool request');
  }
  try {
    const value = JSON.parse(response.message.content);
    if (value && typeof value === 'object' && !Array.isArray(value)) return value;
  } catch {
    throw new Error('Default model returned invalid interview JSON');
  }
  throw new Error('Default model must return a structured interview response');
}

export async function runFleetAssist({
  cwd = process.cwd(), installationRoot = installation, input = process.stdin, output = process.stdout,
  errorOutput = process.stderr, env = process.env, fetchImpl = globalThis.fetch, vault, question,
} = {}) {
  if (!input.isTTY || !output.isTTY) {
    errorOutput.write('roster fleet assist needs a terminal\n');
    return { exitCode: 2, savedProfiles: [] };
  }
  const repoRoot = resolveProjectRoot(cwd);
  const source = await readPrivateFile(repoRoot, 'config.yml');
  if (source === null) throw new Error('roster fleet assist requires .roster/config.yml; run roster onboard first');
  const config = parseConfig(source);
  if (!config.llm.base_url || !config.llm.model ||
      (config.llm.profile !== 'vllm-local' && config.llm.provider !== 'vllm')) {
    throw new Error('roster fleet assist requires an onboarded vLLM base_url and model');
  }
  resolvePublishModel({ config, env: {} });
  if (config.llm.model === 'builtin-stub') throw new Error('Fleet assist needs an actual onboarded model, not a stub');
  const template = await loadInterviewTemplate(installationRoot);
  const controller = new AbortController();
  const terminal = question ? null : createInterface({ input, output, terminal: true });
  terminal?.on('SIGINT', () => controller.abort());
  terminal?.on('close', () => controller.abort());
  const ask = async (prompt) => {
    controller.signal.throwIfAborted();
    const value = await (question ? question(prompt) : terminal.question(prompt, { signal: controller.signal }));
    if (typeof value !== 'string') throw new Error('Fleet interview ended before an answer was supplied');
    return value.trim();
  };
  const request = async (url, options) => {
    const combined = new AbortController();
    const signals = [options.signal, controller.signal].filter(Boolean);
    let abort;
    const cancelled = new Promise((_, reject) => {
      abort = () => { combined.abort(); reject(new DOMException('Interview request cancelled', 'AbortError')); };
      for (const signal of signals) {
        signal.addEventListener('abort', abort, { once: true });
        if (signal.aborted) abort();
      }
    });
    try {
      if (combined.signal.aborted) throw new DOMException('Interview request cancelled', 'AbortError');
      return await Promise.race([fetchImpl(url, { ...options, signal: combined.signal }), cancelled]);
    } finally {
      for (const signal of signals) signal.removeEventListener('abort', abort);
    }
  };
  const chat = createBuiltinChat(config, { fetchImpl: request, env, vault });
  let assisted = true;
  const savedProfiles = [];
  const stop = (value) => ['done', '/quit'].includes(value.toLowerCase());
  const modelFailure = (error) => {
    controller.signal.throwIfAborted();
    if (!(error instanceof Error)) throw error;
    output.write(`Model interview failed: ${redact(error.message, config, env).replace(/\s+/g, ' ').slice(0, 500)}\n` +
      'Falling back to template questions without the model.\n');
    assisted = false;
  };
  const modelResponse = async (payload) => {
    const text = JSON.stringify(payload);
    if (text.length + template.content.length + hardRule.length > config.seat.context_chars) {
      throw new Error('Interview evidence exceeds the configured context character budget');
    }
    return responseObject(await chat({ messages: [
      { role: 'system', content: `${template.content}\n\n${hardRule}` },
      { role: 'user', content: text },
    ] }));
  };
  output.write('Fleet assist: one endpoint at a time. Enter done or /quit to stop. Nothing writes without explicit yes.\n');
  try {
    for (;;) {
      const previous = await readPrivateFile(repoRoot, 'fleet.yml');
      const fleet = previous === null ? validateFleet({ profiles: [] }) : parseFleet(previous);
      const facts = {};
      let models;
      let modelDetails;
      for (const field of fields) {
        if (field === 'context_max' && facts.context_max !== undefined) continue;
        let prompt = template.questions.get(field);
        if (assisted) {
          try {
            const response = await modelResponse({ stage: 'question', field, facts, available_model_ids: models ?? null });
            if (Object.keys(response).join(',') !== 'question' || typeof response.question !== 'string' ||
                !response.question.trim() || response.question.length > 500 ||
                /[\x00-\x1f\x7f]|https?:\/\/|(?:private key|api[_ -]?key|access token|password|\.env\b|pem\b)/i.test(response.question) ||
                (response.question.match(/\?/g) ?? []).length !== 1 || !response.question.trim().endsWith('?') ||
                redact(response.question, config, env) !== response.question) {
              throw new Error('Default model must ask one safe question for the requested field, not propose facts');
            }
            prompt = response.question.trim();
          } catch (error) { modelFailure(error); }
        }
        for (;;) {
          const value = await ask(`${prompt} `);
          if (stop(value)) return { exitCode: 0, savedProfiles };
          try {
            const parsed = parseAnswer(field, value, facts, models, config, env);
            if (field === 'id' && fleet.profiles.some(({ id }) => id === parsed)) {
              throw new TypeError('That profile ID already exists; supply a new ID');
            }
            if (parsed !== undefined) facts[field] = parsed;
            break;
          } catch (error) {
            if (!(error instanceof Error)) throw error;
            output.write(`${redact(error.message, config, env)}\n`);
          }
        }
        if (field === 'base_url') {
          try {
            modelDetails = await probeModelDetails(facts.base_url, {
              fetchImpl: request, env, apiKeyEnv: config.llm.api_key_env,
            });
            models = modelDetails.map(({ id }) => id);
            output.write('Available models:\n' + models.map((id, index) => `  ${index + 1}. ${id}\n`).join(''));
          } catch (error) {
            controller.signal.throwIfAborted();
            if (!(error instanceof Error)) throw error;
            output.write(`Models probe failed: ${redact(error.message, config, env)}. Supply the actual model ID manually.\n`);
          }
        }
        if (field === 'model') {
          const contextMax = modelDetails?.find(({ id }) => id === facts.model)?.context_max;
          if (contextMax !== undefined) {
            facts.context_max = contextMax;
            output.write(`Using context_max=${contextMax} reported by /v1/models for ${facts.model}.\n`);
          }
        }
      }
      const recorded = validateFleetProfile({ ...facts, provider: 'vllm' });
      let proposed = recorded;
      if (assisted) {
        try {
          const response = await modelResponse({ stage: 'proposal', facts: recorded, available_model_ids: models ?? null });
          if (Object.keys(response).join(',') !== 'profile') throw new Error('Default model must return one proposed profile');
          proposed = validateFleetProfile(response.profile);
          if (formatFleet({ profiles: [proposed] }) !== formatFleet({ profiles: [recorded] })) {
            throw new Error('Proposed profile differs from recorded operator/probe facts; invented values are refused');
          }
        } catch (error) {
          modelFailure(error);
          proposed = recorded;
          output.write('Preview below is the validated template profile from recorded answers, not the rejected model proposal.\n');
        }
      }
      output.write(`\nProposed profile:\n${formatFleet({ profiles: [proposed] })}`);
      for (;;) {
        const answer = (await ask('Write this profile? [no] ')).toLowerCase();
        if (stop(answer)) return { exitCode: 0, savedProfiles };
        if (['yes', 'y'].includes(answer)) {
          await writeFleet({ profiles: [...fleet.profiles, proposed] }, { repoRoot, expectedSource: previous });
          savedProfiles.push(proposed.id);
          output.write(`Appended fleet profile: ${proposed.id}. Default config unchanged.\n`);
          break;
        }
        if (!answer || ['no', 'n'].includes(answer)) {
          output.write('Profile not written.\n');
          break;
        }
        output.write('Please answer yes or no; nothing has been written.\n');
      }
    }
  } catch (error) {
    if (error instanceof Error && error.name === 'AbortError') {
      output.write('\nFleet interview cancelled; no unconfirmed profile was written.\n');
      return { exitCode: 0, savedProfiles };
    }
    throw error;
  } finally {
    terminal?.close();
  }
}
