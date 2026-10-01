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
  const initiative = /^(?:build|create|develop|deliver|launch|implement)\s+(?:(?:a|an|the|new|entire|complete|full|standalone|end-to-end)\s+)*(?:orchestrator|platform|product|ecosystem|suite|operating system)\b/;
  if ((requirements.outcomes?.length ?? 0) > 5 || directives.some((line) => initiative.test(line) ||
      /^(?:initiative\b|(?:plan|deliver|launch|build|create)\b.*\b(?:multi-wave|multi-team|initiative)\b)/.test(line))) {
    return { kind: 'initiative', reason: 'Whole-system or multi-wave outcome needs an issue plan' };
  }
  const oneLiner = /\b(?:readme(?:\.md)?\b.*\bone[- ]liner?\b|one[- ]liner?\b.*\breadme(?:\.md)?)\b/.test(summary);
  if ((requirements.outcomes?.length ?? 1) > 1 ||
      !oneLiner && directives.some((line) =>
        /^(?:feature\b|(?:add|build|create|implement|deliver)\b.*\b(?:feature|end-to-end|multi-component)\b)/.test(line))) {
    return { kind: 'feature', reason: 'Feature or multiple outcomes need child issue drafts' };
  }
  if (requirements.files.length || filesAllowed.length || oneLiner) {
    return { kind: 'slice', reason: 'One bounded, named-file outcome can use the sequential seats' };
  }
  return { kind: 'clarify', reason: 'The Ask lacks executable file scope or a clear planning outcome' };
}
