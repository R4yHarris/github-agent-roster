import { askRequirements, cleanAskText } from './stub.mjs';
import { normalizeAsk } from './task.mjs';

export const askKinds = Object.freeze(['clarify', 'slice', 'feature', 'initiative']);

export const clarificationHint = 'Clarify one concrete outcome and name the allowed files, ' +
  'or state a feature or initiative to plan. No seat or implementation ran.';

export function classifyAsk(ask, { title, filesAllowed = [] } = {}) {
  const text = cleanAskText(ask);
  const requirements = askRequirements(title ? `${title}\n${text}` : text, { allowMissing: true });
  const lead = text.replace(/<!--[\s\S]*?-->/g, '').split('\n').map(normalizeAsk).find(Boolean);
  const directives = [title, lead, ...requirements.outcomes ?? []].filter(Boolean).map((line) =>
    normalizeAsk(line).replace(/^(?:feat|fix|docs|test)(?:\([^)]*\))?:\s*/i, '').toLowerCase());
  const summary = directives.join(' ');
  const incident = /\b(?:outage|(?:prod|production)(?:\s+is)?\s+down|regression|hotfix)\b/.test(summary);
  const question = directives.some((line) => /^(?:who|what|when|where|why|how|which|can|could|is|are|does|do)\b/.test(line)) &&
    !/\b(?:add|build|create|develop|deliver|launch|implement|fix|update|change|remove|delete|replace|rewrite|refactor|rename|edit)\b/.test(summary);
  const result = (kind, reason) => ({ kind,
    specKind: incident ? 'incident' : question ? 'question' :
      ({ slice: 'slice', feature: 'story', initiative: 'epic' }[kind] ?? null), reason });
  const initiative = /^(?:build|create|develop|deliver|launch|implement)\s+(?:(?:a|an|the|new|entire|complete|full|standalone|end-to-end)\s+)*(?:orchestrator|platform|product|ecosystem|suite|operating system)\b/;
  const explicitEpic = /^(?:#{1,6}\s*)?(?:epic|initiative)(?:\s+outcome)?\s*:?\s*$/im.test(text) ||
    /^(?:epic|initiative)\s*:/i.test(title ?? '');
  if (explicitEpic || (requirements.outcomes?.length ?? 0) > 5 || directives.some((line) => initiative.test(line) ||
      /^(?:initiative\b|(?:plan|deliver|launch|build|create)\b.*\b(?:multi-wave|multi-team|initiative)\b)/.test(line))) {
    return result('initiative', explicitEpic
      ? 'Explicit epic or initiative outcome needs an issue plan'
      : 'Whole-system or multi-wave outcome needs an issue plan');
  }
  const oneLiner = /\b(?:readme(?:\.md)?\b.*\bone[- ]liner?\b|one[- ]liner?\b.*\breadme(?:\.md)?)\b/.test(summary);
  // A human-declared allow-list, one outcome, and acceptance checks already form a slice (spec 5.3), whatever the title says.
  const acceptance = /^(?:#{1,6}\s*)?acceptance(?:\s+(?:checks|criteria))?\s*:?\s*$/im.test(text);
  if (requirements.explicit && requirements.files.length && acceptance && (requirements.outcomes?.length ?? 1) <= 1) {
    return result('slice', 'Human-declared allow-list, one outcome, and acceptance checks make an executable slice');
  }
  if ((requirements.outcomes?.length ?? 1) > 1 ||
      !oneLiner && directives.some((line) =>
        /^(?:feature\b|(?:add|build|create|implement|deliver)\b.*\b(?:feature|end-to-end|multi-component)\b)/.test(line))) {
    return result('feature', 'Feature or multiple outcomes need child issue drafts');
  }
  if (requirements.files.length || filesAllowed.length || oneLiner) {
    return result('slice', 'One bounded, named-file outcome can use the sequential seats');
  }
  return result('clarify', 'The Ask lacks executable file scope or a clear planning outcome');
}
