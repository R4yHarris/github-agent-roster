const countFields = ['prompt_tokens', 'completion_tokens'];
export const RUN_ENV_NAMES = [
  'AI_PROVIDER', 'AI_MODEL', 'AI_MODEL_VERSION', 'AI_EFFORT', 'AI_CONTEXT_USED',
  'AI_CONTEXT_MAX', 'AI_CONTEXT_OUT', 'AI_SESSION', 'AI_TASK',
];

function isModelId(model) {
  return typeof model === 'string' && /^[A-Za-z0-9][A-Za-z0-9._:+/-]*$/.test(model) &&
    !/^(unknown|none|n\/a|unspecified)$/i.test(model);
}

export function resolvePublishModel({ config, env = process.env } = {}) {
  const model = config?.llm?.model || env.AI_MODEL || env.ROSTER_MODEL;
  if (!isModelId(model)) {
    throw new TypeError('set model: configure llm.model, AI_MODEL, or ROSTER_MODEL with the actual model id (not unknown)');
  }
  return model;
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
  if (config.llm.profile === 'vllm-local') return 'vllm';
  if (config.llm.profile === 'openai') return 'openai';
  if (config.llm.profile === 'ollama' || config.llm.profile === 'lmstudio') return 'local';
  const provider = env.AI_PROVIDER || 'local';
  if (!['vllm', 'github-copilot', 'anthropic', 'openai', 'local', 'other'].includes(provider)) {
    throw new TypeError('Invalid AI_PROVIDER for AI-Run');
  }
  return provider;
}

export function buildRun({ config, usage = {}, session, task, env = process.env }) {
  const model = config.llm.model || env.ROSTER_MODEL;
  if (!model) return null;
  const effort = config.llm.effort;
  const contextMax = config.llm.context_max;
  const version = env.AI_MODEL_VERSION || '-';
  if (!isModelId(model) ||
      typeof version !== 'string' || !/^[A-Za-z0-9._-]+$/.test(version) ||
      !['l', 'm', 'h', 'x', '-'].includes(effort) ||
      !Number.isSafeInteger(contextMax) || contextMax < 0) {
    throw new TypeError('Invalid LLM model, version, effort, or context_max for AI-Run; unknown is not a model');
  }
  const counts = mergeUsage(usage);
  const provider = runProvider(config, env);
  const runEnv = {
    AI_PROVIDER: provider === 'vllm' ? 'local' : provider,
    AI_MODEL: model, AI_MODEL_VERSION: version, AI_EFFORT: effort,
  };
  if (contextMax > 0) runEnv.AI_CONTEXT_MAX = String(contextMax);
  if (counts.prompt_tokens !== undefined) runEnv.AI_CONTEXT_USED = String(counts.prompt_tokens);
  if (counts.completion_tokens !== undefined) runEnv.AI_CONTEXT_OUT = String(counts.completion_tokens);
  for (const [name, value] of [['AI_SESSION', session], ['AI_TASK', task]]) {
    if (value !== undefined) {
      if (typeof value !== 'string' || !/^[A-Za-z0-9._-]{1,64}$/.test(value)) {
        throw new TypeError(`${name} must be an opaque identifier of at most 64 characters`);
      }
      runEnv[name] = value;
    }
  }
  return {
    provider,
    line: [
      '1', runEnv.AI_PROVIDER, `${model}@${version}`, effort,
      `${runEnv.AI_CONTEXT_USED ?? '-'}/${runEnv.AI_CONTEXT_MAX ?? '-'}`,
      runEnv.AI_CONTEXT_OUT ?? '-', runEnv.AI_SESSION ?? '-', runEnv.AI_TASK ?? '-',
    ].join('|'),
    env: runEnv,
  };
}

export function buildPublishEnv({ config, env = process.env, run }) {
  const activeConfig = run?.env?.AI_MODEL
    ? { ...config, llm: { ...config.llm, model: run.env.AI_MODEL } } : config;
  const model = resolvePublishModel({ config: activeConfig, env });
  const publishEnv = { ...env };
  for (const name of RUN_ENV_NAMES) delete publishEnv[name];
  delete publishEnv[config.llm.api_key_env];
  const metadata = run === undefined
    ? buildRun({
      config: { ...activeConfig, llm: { ...activeConfig.llm, model } },
      env, session: env.AI_SESSION, task: env.AI_TASK,
    }) : run;
  Object.assign(publishEnv, metadata?.env);
  if (run === undefined && !config.llm.model && env.AI_MODEL && env.AI_PROVIDER) {
    publishEnv.AI_PROVIDER = env.AI_PROVIDER === 'vllm' ? 'local' : env.AI_PROVIDER;
  }
  publishEnv.AI_MODEL = model;
  return publishEnv;
}
