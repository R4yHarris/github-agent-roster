const countFields = ['prompt_tokens', 'completion_tokens'];

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

export function buildRun({ config, usage = {}, session, task }) {
  if (!config.llm.base_url) return null;
  const { model, effort, context_max: contextMax } = config.llm;
  if (!/^[A-Za-z0-9._:/-]+$/.test(model) || !['l', 'm', 'h', 'x'].includes(effort) ||
      !Number.isSafeInteger(contextMax) || contextMax < 0) {
    throw new TypeError('Invalid LLM model, effort, or context_max for AI-Run');
  }
  const counts = mergeUsage(usage);
  const env = { AI_MODEL: model, AI_EFFORT: effort };
  if (contextMax > 0) env.AI_CONTEXT_MAX = String(contextMax);
  if (counts.prompt_tokens !== undefined) env.AI_CONTEXT_USED = String(counts.prompt_tokens);
  if (counts.completion_tokens !== undefined) env.AI_CONTEXT_OUT = String(counts.completion_tokens);
  for (const [name, value] of [['AI_SESSION', session], ['AI_TASK', task]]) {
    if (value !== undefined) {
      if (typeof value !== 'string' || !/^[A-Za-z0-9._-]{1,64}$/.test(value)) {
        throw new TypeError(`${name} must be an opaque identifier of at most 64 characters`);
      }
      env[name] = value;
    }
  }
  return {
    line: [
      '1', '-', `${model}@unknown`, effort,
      `${env.AI_CONTEXT_USED ?? '-'}/${env.AI_CONTEXT_MAX ?? '-'}`,
      env.AI_CONTEXT_OUT ?? '-', env.AI_SESSION ?? '-', env.AI_TASK ?? '-',
    ].join('|'),
    env,
  };
}
