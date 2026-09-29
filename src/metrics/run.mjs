const countFields = ['prompt_tokens', 'completion_tokens'];
export const RUN_ENV_NAMES = [
  'AI_PROVIDER', 'AI_MODEL', 'AI_MODEL_VERSION', 'AI_EFFORT', 'AI_CONTEXT_USED',
  'AI_CONTEXT_MAX', 'AI_CONTEXT_OUT', 'AI_SESSION', 'AI_TASK',
];

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

export function buildRun({ config, usage = {}, session, task, env = process.env }) {
  const model = config.llm.model || env.ROSTER_MODEL;
  if (!model) return null;
  const effort = config.llm.effort;
  const contextMax = config.llm.context_max;
  const version = env.AI_MODEL_VERSION || '-';
  if (typeof model !== 'string' || model === 'unknown' || !/^[A-Za-z0-9._:/-]+$/.test(model) ||
      typeof version !== 'string' || !/^[A-Za-z0-9._-]+$/.test(version) ||
      !['l', 'm', 'h', 'x', '-'].includes(effort) ||
      !Number.isSafeInteger(contextMax) || contextMax < 0) {
    throw new TypeError('Invalid LLM model, version, effort, or context_max for AI-Run; unknown is not a model');
  }
  const counts = mergeUsage(usage);
  // Contracts v0.2.0 represents vLLM with "local"; "vllm" is not a schema 1 provider.
  const runEnv = {
    AI_PROVIDER: config.llm.profile === 'openai' ? 'openai' : 'local',
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
    line: [
      '1', runEnv.AI_PROVIDER, `${model}@${version}`, effort,
      `${runEnv.AI_CONTEXT_USED ?? '-'}/${runEnv.AI_CONTEXT_MAX ?? '-'}`,
      runEnv.AI_CONTEXT_OUT ?? '-', runEnv.AI_SESSION ?? '-', runEnv.AI_TASK ?? '-',
    ].join('|'),
    env: runEnv,
  };
}

export function buildPublishEnv({ config, env = process.env, run }) {
  const publishEnv = { ...env };
  for (const name of RUN_ENV_NAMES) delete publishEnv[name];
  delete publishEnv[config.llm.api_key_env];
  const metadata = run === undefined
    ? buildRun({ config, env, session: env.AI_SESSION, task: env.AI_TASK }) : run;
  Object.assign(publishEnv, metadata?.env);
  return publishEnv;
}
