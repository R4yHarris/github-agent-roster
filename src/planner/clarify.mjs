import { clarificationHint } from './classify.mjs';
import { cleanAskText } from './stub.mjs';
import { normalizeAsk } from './task.mjs';

const verbs = [
  ['improve|enhance|polish|optimi[sz]e|clean\\s*up|tidy', 'improve'],
  ['fix|repair|resolve|debug', 'fix'],
  ['add|create|build|implement|make', 'add'],
  ['update|change|modify|adjust|tweak', 'change'],
  ['remove|delete|drop', 'remove'],
  ['explain|document|describe', 'document'],
  ['why|what|how|which|where|who|when', 'explain'],
];

const maxQuestions = 3;

function intent(line) {
  const first = line.toLowerCase().replace(/^(?:please|can you|could you|let'?s)\s+/, '');
  for (const [pattern, name] of verbs) {
    if (new RegExp(`^(?:${pattern})\\b`).test(first)) return name;
  }
  return null;
}

function subject(line) {
  const words = line.replace(/[.!?]+$/, '').split(/\s+/).slice(1)
    .filter((word) => !/^(?:the|a|an|this|that|it|things?|stuff|everything|overall|some|my|our)$/i.test(word));
  return words.join(' ').slice(0, 80);
}

// Deterministic, offline best-effort reading of a vague Ask. It never invents files or runs a seat (spec 5.3, 5.7).
export function interpretAmbiguousAsk(ask) {
  const text = cleanAskText(ask);
  const lead = text.replace(/<!--[\s\S]*?-->/g, '').split('\n').map(normalizeAsk).find(Boolean) ?? text;
  const verb = intent(lead);
  const topic = subject(lead);
  const target = topic ? `"${topic}"` : 'the repository';
  const interpretations = {
    improve: `Make one small, reviewable improvement to ${target}.`,
    fix: `Find and fix one defect in ${target} and prove it with a test.`,
    add: `Add one new capability, ${topic ? `"${topic}"` : 'as described'}, as a single slice.`,
    change: `Change one existing behavior of ${target} without widening scope.`,
    remove: `Remove one obsolete piece of ${target} and its references.`,
    document: `Write or correct documentation for ${target}.`,
    explain: `Answer a question about ${target} from repository files; no code change.`,
  };
  const interpretation = interpretations[verb] ?? `Treat "${lead.slice(0, 120)}" as one small slice in ${target}.`;
  const assumptions = [
    'One bounded outcome, not a rewrite or multi-wave initiative.',
    'Only files you name are in scope; Roster will not guess an allow-list.',
    'Existing behavior and tests stay unchanged outside that outcome.',
  ];
  const questions = [
    'Which file(s) or directory should change? (e.g. `README.md`, or `docs/` as an output directory)',
    verb === 'explain' ? 'Which file(s) should the answer be grounded in?'
      : verb === 'fix' ? 'What is the observed failure, and what should happen instead?'
      : 'What single observable result means this is done?',
    'Is this one slice, or a larger feature that should be split into child issues first?',
  ].slice(0, maxQuestions);
  const template = `${lead.replace(/[.!?]+$/, '')}.\n\n## Allowed Files\n- \`<path>\`\n\n## Acceptance checks\n- <one observable result>`;
  return { interpretation, assumptions, questions, template };
}

export function formatClarification(ask) {
  const { interpretation, assumptions, questions, template } = interpretAmbiguousAsk(ask);
  return [
    `Best-effort reading: ${interpretation}`,
    'Assumptions (reply to correct any):',
    ...assumptions.map((line) => `  - ${line}`),
    'Questions (answer one or more, in a follow-up ask):',
    ...questions.map((line, index) => `  ${index + 1}. ${line}`),
    'Ready-to-fill Ask:',
    ...template.split('\n').map((line) => `  | ${line}`),
    'Nothing was guessed or changed; re-submit the Ask with the allowed files filled in to proceed.',
  ].join('\n');
}

export function clarificationResult(ask) {
  return { clarification: clarificationHint, ...interpretAmbiguousAsk(ask),
    guidance: formatClarification(ask) };
}
