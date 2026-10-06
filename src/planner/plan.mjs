import { mergeUsage } from '../metrics/run.mjs';
import { isAllowedFile } from '../runtime/tools.mjs';
import { redactSecrets } from '../runtime/memory.mjs';
import { askRequirements, cleanAskText } from './stub.mjs';
import { allowedFile, checkedList, oneLine } from './task.mjs';
import { selectReasoning } from '../llm/reasoning.mjs';

function object(value, required, optional = []) {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      required.some((key) => !Object.hasOwn(value, key)) ||
      Object.keys(value).some((key) => !required.includes(key) && !optional.includes(key))) {
    throw new TypeError(`PLAN fields must contain ${required.join(', ')}`);
  }
}

// Planner-proposed scope must name tracked files or new files beside tracked ones; it is planned
// scope that tiered expansion and review still govern, never a grant over protected surfaces.
function grounded(file, repositoryFiles) {
  const prefix = file.endsWith('/**') ? file.slice(0, -2) : null;
  if (file === '**/*') return false;
  if (prefix) return repositoryFiles.some((entry) => entry.startsWith(prefix));
  if (repositoryFiles.includes(file)) return true;
  const directory = file.includes('/') ? file.slice(0, file.lastIndexOf('/') + 1) : '';
  return directory !== '' && repositoryFiles.some((entry) => entry.startsWith(directory));
}

export function validatePlan(value, { kind, filesAllowed = [], repositoryFiles, proposedScope = false }) {
  if (!['feature', 'initiative'].includes(kind)) throw new TypeError('PLAN requires a feature or initiative Ask');
  object(value, ['outcomes', 'issues']);
  const outcomes = checkedList(value.outcomes, 'Plan outcomes', (entry) => oneLine(entry, 'Outcome'));
  const maximum = kind === 'feature' ? 5 : 12;
  if (!Array.isArray(value.issues) || value.issues.length < 2 || value.issues.length > maximum) {
    throw new TypeError(`${kind} PLAN must contain 2-${maximum} child issue drafts`);
  }
  const titles = new Set();
  const issues = value.issues.map((issue) => {
    object(issue, ['title', 'outcome', 'acceptance_checks', 'wave'], ['files_allowed']);
    const title = oneLine(issue.title, 'Child issue title');
    if (titles.has(title.toLowerCase())) throw new TypeError('Child issue titles must be distinct');
    titles.add(title.toLowerCase());
    if (!Number.isSafeInteger(issue.wave) || issue.wave < 1 || issue.wave > 8) {
      throw new TypeError('Issue wave must be an integer from 1 to 8');
    }
    const files = issue.files_allowed === undefined || Array.isArray(issue.files_allowed) && !issue.files_allowed.length
      ? [] : checkedList(issue.files_allowed, 'Child issue files', allowedFile, 32);
    const proposed = proposing(filesAllowed, proposedScope, repositoryFiles);
    if (!proposed && files.some((file) => !isAllowedFile(file, filesAllowed))) {
      throw new TypeError('PLAN cannot invent allowed files beyond the human Ask scope');
    }
    if (proposed && Array.isArray(repositoryFiles)) {
      if (!files.length) throw new TypeError(`Child draft "${title}" needs files_allowed so it can run as a slice`);
      const unknown = files.filter((file) => !grounded(file, repositoryFiles));
      if (unknown.length) {
        throw new TypeError(`PLAN files must be tracked repository paths or new files in tracked directories: ${unknown.join(', ')}`);
      }
    }
    return { title, outcome: oneLine(issue.outcome, 'Child issue outcome'),
      acceptance_checks: checkedList(issue.acceptance_checks, 'Child issue checks',
        (check) => oneLine(check, 'Acceptance check')),
      wave: issue.wave, files_allowed: files };
  });
  const waves = [...new Set(issues.map(({ wave }) => wave))].sort((a, b) => a - b);
  if (waves.some((wave, index) => wave !== index + 1)) {
    throw new TypeError('Issue wave labels must start at wave:1 with no gaps');
  }
  return { outcomes, issues, ...(proposing(filesAllowed, proposedScope, repositoryFiles) ? { proposedScope: true } : {}) };
}

function proposing(filesAllowed, proposedScope, repositoryFiles) {
  return !filesAllowed.length && (proposedScope || Array.isArray(repositoryFiles));
}

function planJson(content) {
  const fenced = content.match(/```(?:json)?\s*([\s\S]*?)```/)?.[1]?.trim();
  const text = fenced || content.trim();
  try { return JSON.parse(text); } catch (error) {
    const start = text.indexOf('{');
    const end = text.lastIndexOf('}');
    if (start < 0 || end <= start) throw error;
    return JSON.parse(text.slice(start, end + 1));
  }
}

function stubPlan(ask, kind, filesAllowed) {
  const requirements = askRequirements(ask, { allowMissing: true });
  const outcome = cleanAskText(ask).split('\n').find((line) => line.trim() && !line.startsWith('#')) ?? ask;
  const outcomes = requirements.outcomes ?? [outcome.slice(0, 240)];
  const drafts = outcomes.length > 1 ? outcomes.map((entry, index) => ({
    title: `Slice ${index + 1}: ${entry}`.slice(0, 240), outcome: entry, wave: index + 1,
    acceptance_checks: ['The named outcome has passing acceptance evidence',
      'Allowed files are explicitly agreed before an executable TASK is written'],
  })) : [
    { title: 'Define the first bounded outcome and file scope', outcome: outcomes[0], wave: 1,
      acceptance_checks: ['A human agrees one outcome, allowed files, and acceptance checks'] },
    { title: 'Deliver and verify the agreed slice', outcome: outcomes[0], wave: 2,
      acceptance_checks: ['The agreed slice passes tests, excellence, and read-only review'] },
  ];
  if (kind === 'initiative') drafts.push({
    title: 'Verify integrated outcomes and remaining waves',
    outcome: 'Confirm the initiative outcomes with linked issue and PR evidence', wave: Math.min(drafts.length + 1, 8),
    acceptance_checks: ['Each outcome has issue and reviewed PR evidence', 'Remaining work stays in GitHub issues'],
  });
  return validatePlan({ outcomes, issues: drafts.map((issue) => ({ ...issue, files_allowed: filesAllowed })) },
    { kind, filesAllowed });
}

export function renderPlan(plan, { ask, title, kind, reference }) {
  const heading = oneLine(title ?? cleanAskText(ask).split('\n')[0].replace(/^#+\s*/, '').slice(0, 240), 'Plan title');
  const waves = [...new Set(plan.issues.map(({ wave }) => wave))].sort((a, b) => a - b);
  const text = `# Plan: ${heading}\n\nAsk kind: ${kind}\nReference: ${oneLine(reference, 'Plan reference')}\n\n` +
    (plan.proposedScope ? 'Scope: planner-proposed\n\n' : '') +
    'Planning only. Child issues below are drafts, not created issues or executable tasks. ' +
    (plan.issues.every((issue) => issue.files_allowed.length)
      ? 'An issue run opens them as linked GitHub issues and continues with the first open wave slice; ' +
        'planned files are scope that tiered expansion and review govern.\n\n'
      : 'Review and create them on GitHub, then /run each bounded slice separately. No coder or publisher ran.\n\n') +
    '## Outcomes\n\n' + plan.outcomes.map((outcome) => `- ${outcome}`).join('\n') +
    '\n\n## Waves\n\n' + waves.map((wave) =>
      `- \`wave:${wave}\`: drafts ${plan.issues.flatMap((issue, index) => issue.wave === wave ? [index + 1] : []).join(', ')}`
    ).join('\n') + '\n\nWaves are issue labels, not a separate queue or database.\n\n## Child issue drafts\n\n' +
    plan.issues.map((issue, index) => `### Draft ${index + 1}: ${issue.title}\n\n` +
      `Labels: \`wave:${issue.wave}\`\n\nOutcome: ${issue.outcome}\n\nAcceptance checks:\n` +
      issue.acceptance_checks.map((check) => `- ${check}`).join('\n') + '\n\nAllowed files:\n' +
      (issue.files_allowed.length ? issue.files_allowed.map((file) => `- \`${file}\``).join('\n')
        : '- Human must name allowed files before this draft becomes an executable TASK.')
    ).join('\n\n') + `\n\n## Original Ask\n\n${cleanAskText(ask)}\n`;
  if (Buffer.byteLength(text, 'utf8') > 65_536) throw new TypeError('PLAN.md must be at most 64 KiB');
  return text;
}

export async function planOutline(ask, {
  kind, config, title, reference = 'local:draft', fetchImpl, env, vault, onEvent, onResponse, retryCommand, signal,
  repositoryFiles,
} = {}) {
  const text = cleanAskText(ask);
  config = selectReasoning(config, { kind });
  if (!['feature', 'initiative'].includes(kind)) throw new TypeError('PLAN requires a feature or initiative Ask');
  const filesAllowed = askRequirements(title ? `${title}\n${text}` : text, { allowMissing: true }).files;
  const proposeScope = !filesAllowed.length && Array.isArray(repositoryFiles) && repositoryFiles.length > 0;
  const finish = (plan, evidence) => ({
    ...evidence, outline: plan, plan: renderPlan(plan, { ask: text, title, kind, reference }),
    askKind: kind, planningOnly: true,
  });
  if (!config.llm.base_url) return finish(stubPlan(text, kind, filesAllowed),
    { mode: 'stub', turns: 0, usage: null, response: null });
  const budget = config.planner?.turn_budget;
  if (!Number.isSafeInteger(budget) || budget < 1 || budget > 64) {
    throw new TypeError('Planner turn budget must be between 1 and 10000');
  }
  const messages = [
    { role: 'system', content: `You are the builtin ${kind} planner seat. Plan only; you have no tools. ` +
      'Do not implement, create issues, or grant policy permissions. Return only JSON with outcomes (1-8 short lines) ' +
      `and issues (${kind === 'feature' ? '2-5' : '2-12'} child issue drafts). Each issue has title, outcome, ` +
      'acceptance_checks (1-8 short verifiable lines), wave (integer1-8 starting at1 without gaps), ' +
      (proposeScope
        ? 'and files_allowed (1-32 paths). The human named no files: propose each draft\'s planned files from ' +
          'repository_files, or new files inside their directories. Planned files are scope the coder may expand ' +
          'with review; never name workflows, policy, secrets, or vendor sources. Order waves so each draft can ' +
          'be delivered and tested on its own. '
        : 'and optional files_allowed. Files may only come from the human-named scope; omit them when scope is unknown. ') +
      'Waves will be issue labels wave:N, not a new board. The harness writes PLAN.md only. ' +
      'Use validation feedback to change the plan rather than repeating an invalid answer.' },
    { role: 'user', content: JSON.stringify({ title, ask: text, human_files_allowed: filesAllowed,
      ...(proposeScope ? { repository_files: repositoryFiles } : {}) }) },
  ];
  const attempts = Math.min(budget, 4);
  const usages = [];
  let response;
  for (let turn = 1; turn <= attempts; turn += 1) {
    const completion = await (await import('../lib/llm.mjs')).chatCompletion({ config, messages, fetchImpl, env, vault, onEvent, retryCommand, signal, stream: true });
    response = completion.response;
    onResponse?.(response);
    usages.push(completion.usage);
    const choice = completion.choices?.[0];
    let validated;
    let failure;
    try {
      if (choice?.finish_reason != null && choice.finish_reason !== 'stop' ||
          choice?.message?.tool_calls?.length ||
          typeof choice?.message?.content !== 'string' ||
          Buffer.byteLength(choice.message.content, 'utf8') > 32_768) {
        throw new TypeError('Planning-only seat must return JSON, not tools or implementation');
      }
      validated = validatePlan(planJson(choice.message.content), { kind, filesAllowed,
        ...(proposeScope ? { repositoryFiles } : {}) });
    } catch (error) {
      if (!(error instanceof TypeError || error instanceof SyntaxError)) throw error;
      failure = error instanceof SyntaxError ? 'PLAN response must be valid JSON'
        : redactSecrets(error.message, { env, apiKeyEnv: config.llm.api_key_env });
    }
    if (validated) return finish(validated,
      { mode: 'llm', turns: turn, usage: mergeUsage(...usages), response });
    if (turn === attempts) {
      throw new Error(`Planning-only ${kind} failed after ${attempts} attempts: ${failure}. No coder or publisher ran.`);
    }
    const previous = typeof choice?.message?.content === 'string' ? choice.message.content.slice(0, 16_384) : '';
    messages.push({ role: 'assistant', content: redactSecrets(previous, { env, apiKeyEnv: config.llm.api_key_env }) });
    messages.push({ role: 'user', content: `Emit only the requested PLAN JSON. Validation error: ${failure}. ` +
      'Fix exactly that problem; do not repeat the rejected answer.' });
  }
}
