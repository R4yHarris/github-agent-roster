import { isLocalLlmHost } from './request.mjs';

const cloudEfforts = { l: 'low', m: 'medium', h: 'high', x: 'xhigh', none: 'none' };
const deepseekEfforts = { l: 'low', m: 'high', h: 'high', x: 'max', none: 'none' };

export function usesDeepseekReasoning(llm) {
  return Boolean(llm.base_url && isLocalLlmHost(new URL(llm.base_url).hostname) &&
    /deepseek[-_/]?v4[._-]?1/i.test(llm.model ?? ''));
}

export function mappedEffort(llm, effort = llm.effort ?? 'm') {
  const result = (usesDeepseekReasoning(llm) ? deepseekEfforts : cloudEfforts)[effort];
  if (!result) throw new TypeError('Reasoning effort must be l, m, h, x, or none');
  return result;
}

export function nextEffort(llm, effort) {
  if (effort === 'none') return 'none';
  const tiers = usesDeepseekReasoning(llm) ? ['l', 'h', 'x'] : ['l', 'm', 'h', 'x'];
  const normalized = usesDeepseekReasoning(llm) && effort === 'm' ? 'h' : effort;
  const index = tiers.indexOf(normalized);
  if (index < 0) throw new TypeError('Prior reasoning effort is invalid');
  return tiers[Math.min(index + 1, tiers.length - 1)];
}

export function selectReasoning(config, { kind, taskClass, difficulty, previousEffort } = {}) {
  previousEffort ??= config.llm.review_retry_effort;
  const docsSlice = kind === 'slice' && taskClass === 'docs' && [1, 2].includes(difficulty);
  const planning = ['feature', 'initiative'].includes(kind);
  let effort = config.llm.effort_override ?? (docsSlice ? 'l' : planning ? 'h' : config.llm.effort);
  if (previousEffort !== undefined && config.llm.effort_override === undefined) {
    effort = nextEffort(config.llm, previousEffort);
  }
  if (usesDeepseekReasoning(config.llm) && effort === 'm') effort = 'h';
  return { ...config, llm: { ...config.llm, effort,
    ...(docsSlice ? { max_tokens: 2048 } : planning ? { max_tokens: 4096 } : {}),
  } };
}
