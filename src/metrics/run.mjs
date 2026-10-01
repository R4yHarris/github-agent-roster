import { requirePublicationEnabled, withoutLlmKeys } from '../lib/config.mjs';

const countFields = ['prompt_tokens', 'completion_tokens'];
export const RUN_ENV_NAMES = [
  'AI_PROVIDER', 'AI_MODEL', 'AI_MODEL_VERSION', 'AI_EFFORT', 'AI_CONTEXT_USED',
  'AI_CONTEXT_MAX', 'AI_CONTEXT_OUT', 'AI_SESSION', 'AI_TASK',
];

function isModelId(model) {
  return typeof model === 'string' && /^[A-Za-z0-9][A-Za-z0-9._:+/-]*$/.test(model) &&
    !/^(unknown|none|n\/a|unspecified)$/i.test(model);
}

export function resolvePublishModel({ config, env = process.env, model, ghcp = false } = {}) {
  const selected = ghcp ? model ?? env.AI_MODEL : config?.llm?.model || env.AI_MODEL || env.ROSTER_MODEL;
  if (!isModelId(selected)) {
    throw new TypeError(ghcp
      ? 'set model: GHCP publication requires --model or AI_MODEL with the actual model id (not unknown)'
      : 'set model: configure llm.model, AI_MODEL, or ROSTER_MODEL with the actual model id (not unknown)');
  }
  return selected;
}

export function normalizeRunEffort(value) {
  if (value === undefined || value === '') return '-';
  const aliases = { low: 'l', medium: 'm', high: 'h', max: 'x', l: 'l', m: 'm', h: 'h', x: 'x', '-': '-' };
  if (typeof value !== 'string' || !Object.hasOwn(aliases, value)) {
    throw new TypeError('AI_EFFORT must be l, m, h, x, low, medium, high, or max');
  }
  return aliases[value];
}

export function mergeUsage(...samples) {
  const totals = {};
  for (const field of countFields) {
    let sum = 0;
    let known = samples.length > 0;
    for (const sample of samples) {
      const count = sample?.[field];
      if (count === undefined || count === null) {
        known = false;
      } else if (!Number.isSafeInteger(count) || count < 0) {
        throw new TypeError(`LLM ${field} must be a nonnegative safe integer`);
      } else {
        sum += count;
        if (!Number.isSafeInteger(sum)) throw new TypeError(`LLM ${field} total exceeds safe integer range`);
      }
    }
    if (known) totals[field] = sum;
  }
  return totals;
}

function runProvider(config, env) {
  if (config.llm.provider) {
    if (!['vllm', 'github-copilot', 'anthropic', 'openai', 'local', 'other'].includes(config.llm.provider)) {
      throw new TypeError('Invalid llm.provider for AI-Run');
    }
    return config.llm.provider;
  }
  if (config.llm.profile === 'vllm-local') return 'vllm';
  if (config.llm.profile === 'openai') return 'openai';
  if (config.llm.profile === 'ollama' || config.llm.profile === 'lmstudio') return 'local';
  const provider = env.AI_PROVIDER || 'local';
  if (!['vllm', 'github-copilot', 'anthropic', 'openai', 'local', 'other'].includes(provider)) {
    throw new TypeError('Invalid AI_PROVIDER for AI-Run');
  }
  return provider;
}

export function materializeRun(metrics, version = '-') {
  if (!metrics || typeof metrics !== 'object' || Array.isArray(metrics) ||
      !isModelId(metrics.model) ||
      !['vllm', 'github-copilot', 'anthropic', 'openai', 'local', 'other'].includes(metrics.provider) ||
      typeof version !== 'string' || !/^[A-Za-z0-9._-]+$/.test(version) ||
      !['l', 'm', 'h', 'x', '-'].includes(metrics.effort) ||
      (metrics.context_max !== undefined &&
        !(Number.isSafeInteger(metrics.context_max) && metrics.context_max > 0 ||
          typeof metrics.context_max === 'string' && /^[1-9]\d*$/.test(metrics.context_max)))) {
    throw new TypeError('Invalid LLM model, version, effort, or context_max for AI-Run; unknown is not a model');
  }
  const counts = mergeUsage(metrics);
  for (const [alias, field] of [['context_used', 'prompt_tokens'], ['context_out', 'completion_tokens']]) {
    if (metrics[alias] !== undefined && metrics[alias] !== counts[field]) {
      throw new TypeError('Run token aliases must match the reported response usage');
    }
  }
  const record = {
    provider: metrics.provider, model: metrics.model, effort: metrics.effort,
    ...(metrics.context_max === undefined ? {} : { context_max: metrics.context_max }),
    ...(counts.prompt_tokens === undefined ? {} : {
      prompt_tokens: counts.prompt_tokens, context_used: counts.prompt_tokens,
    }),
    ...(counts.completion_tokens === undefined ? {} : {
      completion_tokens: counts.completion_tokens, context_out: counts.completion_tokens,
    }),
  };
  const runEnv = {
    AI_PROVIDER: record.provider === 'vllm' ? 'local' : record.provider,
    AI_MODEL: record.model, AI_MODEL_VERSION: version, AI_EFFORT: record.effort,
  };
  if (record.context_max !== undefined) runEnv.AI_CONTEXT_MAX = String(record.context_max);
  if (counts.prompt_tokens !== undefined) runEnv.AI_CONTEXT_USED = String(counts.prompt_tokens);
  if (counts.completion_tokens !== undefined) runEnv.AI_CONTEXT_OUT = String(counts.completion_tokens);
  for (const [name, field] of [['AI_SESSION', 'session'], ['AI_TASK', 'task']]) {
    const value = metrics[field];
    if (value !== undefined) {
      if (typeof value !== 'string' || !/^[A-Za-z0-9._-]{1,64}$/.test(value)) {
        throw new TypeError(`${name} must be an opaque identifier of at most 64 characters`);
      }
      runEnv[name] = value;
      record[field] = value;
    }
  }
  return Object.freeze({
    provider: record.provider, metrics: Object.freeze(record), version,
    line: [
      '1', runEnv.AI_PROVIDER, `${record.model}@${version}`, record.effort,
      `${runEnv.AI_CONTEXT_USED ?? '-'}/${runEnv.AI_CONTEXT_MAX ?? '-'}`,
      runEnv.AI_CONTEXT_OUT ?? '-', runEnv.AI_SESSION ?? '-', runEnv.AI_TASK ?? '-',
    ].join('|'),
    env: Object.freeze(runEnv),
  });
}

export function buildRun({ config, usage = {}, response, session, task, env = process.env }) {
  const live = response !== undefined;
  const model = response?.model ?? (config.llm.model || (!live ? env.ROSTER_MODEL : undefined));
  if (!model) return null;
  const contextMax = config.llm.context_max;
  if (contextMax !== undefined && (!Number.isSafeInteger(contextMax) || contextMax < 0)) {
    throw new TypeError('Invalid context_max for AI-Run');
  }
  const counts = mergeUsage(live ? response?.usage ?? {} : usage);
  return materializeRun({
    provider: runProvider(config, live ? {} : env), model, effort: config.llm.effort === 'none' ? '-' : config.llm.effort,
    ...(contextMax > 0 ? { context_max: contextMax } : {}),
    ...counts, ...(session === undefined ? {} : { session }), ...(task === undefined ? {} : { task }),
  }, live ? '-' : env.AI_MODEL_VERSION || '-');
}

export function buildGhcpRun({ env = process.env, model, session, task } = {}) {
  const actualModel = resolvePublishModel({ env, model, ghcp: true });
  const effort = normalizeRunEffort(env.AI_EFFORT);
  const identity = session ?? (typeof env.AI_SESSION === 'string' && env.AI_SESSION.startsWith('ghcp-')
    ? env.AI_SESSION : `ghcp-${process.pid}`);
  if (typeof identity !== 'string' || !/^ghcp-[A-Za-z0-9._-]+$/.test(identity)) {
    throw new TypeError('GHCP AI_SESSION must use a ghcp- date or process identifier');
  }
  let capacity;
  if (env.AI_CONTEXT_MAX !== undefined && env.AI_CONTEXT_MAX !== '' && env.AI_CONTEXT_MAX !== '-') {
    if (typeof env.AI_CONTEXT_MAX !== 'string' || !/^[1-9]\d*$/.test(env.AI_CONTEXT_MAX)) {
      throw new TypeError('GHCP-only AI_CONTEXT_MAX must be a positive decimal integer');
    }
    const count = Number(env.AI_CONTEXT_MAX);
    capacity = Number.isSafeInteger(count) ? count : env.AI_CONTEXT_MAX;
  }
  const taskId = task ?? env.AI_TASK;
  return materializeRun({
    provider: 'github-copilot', model: actualModel, effort, session: identity,
    ...(capacity === undefined ? {} : { context_max: capacity }),
    ...(taskId === undefined || taskId === '' ? {} : { task: taskId }),
  });
}

export function buildPublishEnv({ config, env = process.env, run, model, session, task }) {
  requirePublicationEnabled(config);
  const metadata = run?.metrics ? materializeRun(run.metrics, run.version)
    : run ?? buildGhcpRun({ env, model, session, task });
  const actualModel = resolvePublishModel({ env: { AI_MODEL: metadata.env?.AI_MODEL } });
  const publishEnv = withoutLlmKeys(env, config);
  for (const name of RUN_ENV_NAMES) delete publishEnv[name];
  Object.assign(publishEnv, metadata.env);
  publishEnv.AI_MODEL = actualModel;
  return publishEnv;
}
