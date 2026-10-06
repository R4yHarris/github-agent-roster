import { askRequirements } from './stub.mjs';
import { validatePlan } from './plan.mjs';

export function parsePlanDocument(source) {
  if (typeof source !== 'string' || Buffer.byteLength(source) > 65536) throw new TypeError('PLAN.md must be at most 64 KiB');
  const text = source.replaceAll('\r\n', '\n');
  const kind = /^Ask kind: (feature|initiative)$/m.exec(text)?.[1];
  const reference = /^Reference: (issue:[1-9]\d*|local:[A-Za-z0-9._-]+)$/m.exec(text)?.[1];
  const outcomes = /## Outcomes\n\n([\s\S]*?)\n\n## Waves\n/.exec(text)?.[1];
  const children = /## Child issue drafts\n\n([\s\S]*?)\n\n## Original Ask\n\n([\s\S]+)$/.exec(text);
  if (!kind || !reference || !outcomes || !children) throw new Error('Current PLAN.md has no valid child wave drafts');
  const list = (value) => value.trim().split('\n').map((line) => {
    const match = /^- (.+)$/.exec(line);
    if (!match) throw new Error('Plan lists must contain explicit bullet entries');
    return match[1].replace(/^`(.+)`$/, '$1');
  });
  const issues = children[1].split(/(?=^### Draft [1-9]\d*:)/m).filter((entry) => entry.trim()).map((entry, index) => {
    const match = /^### Draft ([1-9]\d*): ([^\n]+)\n\nLabels: `wave:([1-8])`\n\nOutcome: ([^\n]+)\n\nAcceptance checks:\n([\s\S]*?)\n\nAllowed files:\n([\s\S]+)$/.exec(entry.trim());
    if (!match || Number(match[1]) !== index + 1) throw new Error('Plan draft numbering or fields are invalid');
    return { title: match[2], wave: Number(match[3]), outcome: match[4], acceptance_checks: list(match[5]),
      files_allowed: match[6].trim() === '- Human must name allowed files before this draft becomes an executable TASK.'
        ? [] : list(match[6]) };
  });
  const ask = children[2].trim();
  return { kind, reference, ask, ...validatePlan({ outcomes: list(outcomes), issues }, {
    kind, filesAllowed: askRequirements(ask, { allowMissing: true }).files,
    proposedScope: /^Scope: planner-proposed$/m.test(text.split('\n## Outcomes\n')[0]),
  }) };
}
