import { createHash } from 'node:crypto';
import path from 'node:path';
import { isManagedFile } from './tools.mjs';
import { redactEvidence } from '../lib/redaction.mjs';
import { parseTaskDocument, taskSections } from '../planner/task.mjs';

// Spec §5.4: the coder works the TASK.md checks as a checklist and cannot finish while any item is open.
const statuses = ['pending', 'in_progress', 'done', 'blocked'];
const oneLine = (value) => String(value ?? '').replace(/\s+/g, ' ').trim().slice(0, 300);

export const checklistTool = {
  type: 'function',
  function: {
    name: 'update_checklist',
    description: 'Update your checklist of TASK.md acceptance checks. Mark one item in_progress while you work on it, ' +
      'then done with evidence (the file and symbol or test that proves it), or blocked with the reason. ' +
      'You cannot finish while any item is pending or in_progress.',
    parameters: {
      type: 'object',
      properties: {
        items: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              id: { type: 'integer', description: 'Check number from TASK.md' },
              status: { type: 'string', enum: statuses },
              evidence: { type: 'string', description: 'Required for done and blocked' },
            },
            required: ['id', 'status'],
          },
        },
      },
      required: ['items'],
    },
  },
};

export function createChecklist(checks, { exact = false } = {}) {
  return { items: checks.map((check, index) => ({ id: index + 1, check: exact ? check : oneLine(check), status: 'pending', evidence: '' })) };
}

const digest = (value) => createHash('sha256').update(value).digest('hex');
const capsuleLimit = 65536;
const observationLimit = 32;
const observedTools = ['read_file', 'write_file', 'edit_file', 'delete_file', 'run_test'];
const identity = (task, worktree) => ({ task: digest(task), worktree: digest(path.resolve(worktree)) });
const failContinuation = (reason) => { throw new TypeError(`Acceptance continuation: ${reason}`); };
const keysAre = (value, keys) => value && typeof value === 'object' && !Array.isArray(value) &&
  Object.keys(value).sort().join(',') === keys.split(',').sort().join(',');
const hash = (value) => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);

// The planner's display parser caps each check at 240 characters; continuity must not lose obligations.
export function authoritativeAcceptanceChecks(task) {
  const parsed = parseTaskDocument(task);
  const content = taskSections(task).sections.find(({ name }) => name === 'acceptance checks').content;
  const checks = [];
  for (const line of content.split('\n').filter((line) => line.trim())) {
    const text = line.trim().replace(/^(?:[-*+]|\d+[.)])[ \t]+/, '');
    if (/^\s+\S/.test(line) && checks.length) checks[checks.length - 1] += ` ${text}`;
    else checks.push(text.replace(/^`([^`]+)`$/, '$1'));
  }
  if (checks.length !== parsed.acceptance_checks.length) failContinuation('authoritative check count mismatch');
  return checks;
}

// Reuse the excellence snapshot: generated seat artifacts are not product bytes.
export function acceptanceSourceIdentity(snapshot) {
  return digest(JSON.stringify([...snapshot].filter(([file]) => !isManagedFile(file))
    .sort(([a], [b]) => a.localeCompare(b))));
}

export function acceptanceObservation({ id, tool, args, result, source, env, apiKeyEnv }) {
  if (!observedTools.includes(tool)) return null;
  if (tool === 'run_test' && (!Number.isSafeInteger(result?.exit_code) || result.exit_code < 0)) {
    failContinuation('run_test observation needs an observed exit code');
  }
  const reference = tool === 'run_test' ? 'node --test'
    : redactEvidence(String(result?.path ?? args?.path ?? '').replaceAll('\\', '/'), { env, apiKeyEnv });
  if (!reference || reference.length > 300) failContinuation('tool reference exceeds bounds');
  return { id, tool, reference, source, verdict: tool === 'run_test'
    ? result.exit_code === 0 ? 'pass' : 'fail' : 'observed' };
}

export function acceptanceUpdateReferences(args, observations, source) {
  return (args?.items ?? []).map((update) => {
    if (!['done', 'blocked'].includes(update.status)) return null;
    const evidence = oneLine(update.evidence);
    const observed = [...observations].reverse().find((entry) => entry.source === source &&
      (evidence.includes(entry.reference) || entry.tool === 'run_test' && evidence.includes('run_test')) &&
      (update.status === 'done' ? entry.verdict !== 'fail' : entry.verdict === 'fail'));
    if (!observed) failContinuation(`check ${update.id} needs current observed ${update.status === 'done' ? 'evidence' : 'failure evidence'}`);
    return observed.id;
  });
}

export function acceptanceReferenceEvidence(observation) {
  return `Observation ${observation.id}: ${observation.tool} ${JSON.stringify(observation.reference)} (${observation.verdict})`;
}

export function renderAcceptanceChecklist(checklist, observations) {
  return checklist.items.map(({ id, check, status, evidenceRef }) => {
    const observation = observations.find((entry) => entry.id === evidenceRef);
    return `${id}. [${status}] ${check}${observation
      ? ` (evidence: ${acceptanceReferenceEvidence(observation)})` : ''}`;
  }).join('\n');
}

export function exportAcceptanceContinuation({ task, worktree, checklist, observations, source, outcome, env, apiKeyEnv,
  priorNotices = [] }) {
  if (!checklist) return null;
  const retained = observations.filter((entry) => entry.source === source).slice(-observationLimit);
  const notices = [];
  const items = checklist.items.map((item) => {
    let { status } = item;
    let evidence = '';
    let evidenceRef = item.evidenceRef ?? null;
    const observation = retained.find(({ id }) => id === evidenceRef);
    if (['done', 'blocked'].includes(status) &&
        (!observation || status === 'done' && (outcome !== 'verified' || observation.verdict === 'fail') ||
          status === 'blocked' && observation.verdict !== 'fail')) {
      notices.push(`Check ${item.id}: ${status} invalidated; evidence is missing, changed, or the attempt was not verified.`);
      status = 'pending';
      evidence = '';
      evidenceRef = null;
    }
    if (['done', 'blocked'].includes(status)) evidence = acceptanceReferenceEvidence(observation);
    if (!['done', 'blocked'].includes(status)) { evidence = ''; evidenceRef = null; }
    return { id: item.id, check: item.check, status, evidence, evidenceRef };
  });
  const combined = [...new Set([...priorNotices, ...notices])].slice(-checklist.items.length);
  const capsule = { version: 1, ...identity(task, worktree), source, outcome, items, observations: retained, notices: combined };
  if (JSON.stringify(capsule).length > capsuleLimit) failContinuation('capsule exceeds 65536 characters');
  return capsule;
}

export function importAcceptanceContinuation(capsule, { task, worktree, checks, source, env, apiKeyEnv }) {
  if (!keysAre(capsule, 'version,task,worktree,source,outcome,items,observations,notices') ||
      JSON.stringify(capsule).length > capsuleLimit || capsule.version !== 1 ||
      !['verified', 'failed', 'cancelled'].includes(capsule.outcome)) failContinuation('malformed capsule');
  const expected = identity(task, worktree);
  if (capsule.task !== expected.task || capsule.worktree !== expected.worktree) failContinuation('TASK or worktree mismatch');
  if (!hash(capsule.source) || capsule.source !== source) failContinuation('stale source bytes; rerun checks');
  if (!Array.isArray(capsule.items) || capsule.items.length > checks.length ||
      !Array.isArray(capsule.observations) || capsule.observations.length > observationLimit ||
      !Array.isArray(capsule.notices) || capsule.notices.length > checks.length ||
      capsule.notices.some((notice) => typeof notice !== 'string' || notice.length > 300)) failContinuation('invalid bounded entries');
  const ids = new Set();
  for (const entry of capsule.observations) {
    if (!keysAre(entry, 'id,tool,reference,source,verdict') || !Number.isSafeInteger(entry.id) || entry.id < 1 ||
        ids.has(entry.id) || !observedTools.includes(entry.tool) || entry.source !== source ||
        typeof entry.reference !== 'string' || !entry.reference || entry.reference.length > 300 ||
        (entry.tool === 'run_test' ? !['pass', 'fail'].includes(entry.verdict) || entry.reference !== 'node --test'
          : entry.verdict !== 'observed')) failContinuation('invalid observation reference');
    ids.add(entry.id);
  }
  const checklist = createChecklist(checks, { exact: true });
  const seen = new Set();
  for (const entry of capsule.items) {
    const item = checklist.items.find(({ id }) => id === entry?.id);
    if (!keysAre(entry, 'id,check,status,evidence,evidenceRef') || !item || seen.has(entry.id) ||
        entry.check !== item.check || !statuses.includes(entry.status) ||
        typeof entry.evidence !== 'string' || entry.evidence.length > 400) failContinuation('malformed or mismatched obligation');
    if (['done', 'blocked'].includes(entry.status)) {
      const observation = capsule.observations.find(({ id }) => id === entry.evidenceRef);
      if (!entry.evidence || !observation || entry.status === 'done' &&
          (capsule.outcome !== 'verified' || observation.verdict === 'fail') ||
          entry.status === 'blocked' && observation.verdict !== 'fail' ||
          entry.evidence !== acceptanceReferenceEvidence(observation)) failContinuation('terminal obligation lacks valid observed evidence');
    } else if (entry.evidenceRef !== null || entry.evidence) failContinuation('open obligation has terminal evidence');
    seen.add(entry.id);
    Object.assign(item, entry, { evidence: ['done', 'blocked'].includes(entry.status)
      ? acceptanceReferenceEvidence({ ...capsule.observations.find(({ id }) => id === entry.evidenceRef),
        reference: redactEvidence(capsule.observations.find(({ id }) => id === entry.evidenceRef).reference,
          { env, apiKeyEnv }) }) : '' });
  }
  if (checklist.items.filter(({ status }) => status === 'in_progress').length > 1) failContinuation('multiple in_progress obligations');
  return { checklist, observations: capsule.observations.map((entry) => ({ ...entry,
    reference: redactEvidence(entry.reference, { env, apiKeyEnv }) })),
  notices: capsule.notices.map((notice) => redactEvidence(notice, { env, apiKeyEnv })) };
}

export const openItems = (checklist) => checklist.items.filter(({ status }) => ['pending', 'in_progress'].includes(status));

export function checklistProgress(checklist) {
  return { done: checklist.items.filter(({ status }) => status === 'done').length, total: checklist.items.length };
}

export function renderChecklist(checklist) {
  return checklist.items.map(({ id, check, status, evidence }) =>
    `${id}. [${status}] ${check}${evidence ? ` (evidence: ${evidence})` : ''}`).join('\n');
}

// Applies one update_checklist call; throws TypeError for invalid updates so the coder gets a correction.
export function updateChecklist(checklist, args) {
  const updates = args?.items;
  if (!Array.isArray(updates) || !updates.length || updates.length > checklist.items.length) {
    throw new TypeError('update_checklist needs items: [{id, status, evidence}] for existing check numbers');
  }
  const next = checklist.items.map((item) => ({ ...item }));
  for (const update of updates) {
    const item = next.find(({ id }) => id === update?.id);
    if (!item) throw new TypeError(`update_checklist: no check ${JSON.stringify(update?.id)}; use 1-${next.length}`);
    if (!statuses.includes(update.status)) throw new TypeError(`update_checklist: status must be one of ${statuses.join(', ')}`);
    const evidence = oneLine(update.evidence);
    if (['done', 'blocked'].includes(update.status) && !evidence) {
      throw new TypeError(`update_checklist: check ${item.id} marked ${update.status} needs evidence`);
    }
    Object.assign(item, { status: update.status, evidence: evidence || item.evidence });
  }
  if (next.filter(({ status }) => status === 'in_progress').length > 1) {
    throw new TypeError('update_checklist: work one item at a time; at most one item may be in_progress');
  }
  checklist.items = next;
  return `Checklist updated.\n${renderChecklist(checklist)}`;
}

// Closes items a final summary reports per check ("1. done: evidence", "Check 2 - blocked: reason").
export function closeFromSummary(checklist, summary) {
  for (const line of String(summary ?? '').split(/\r?\n/)) {
    const match = /^\s*(?:[-*]\s*)?(?:check\s*)?#?(\d+)\s*[.):-]?\s*\[?(done|met|blocked)\]?\s*[-:—]?\s*(.*)$/i.exec(line);
    const item = match && checklist.items.find(({ id }) => id === Number(match[1]));
    if (!item || !['pending', 'in_progress'].includes(item.status)) continue;
    item.status = /blocked/i.test(match[2]) ? 'blocked' : 'done';
    item.evidence = oneLine(match[3]) || 'stated in the final summary';
  }
  return checklist;
}

export function openChecklistCorrection(checklist) {
  return `Your checklist still has open items:\n${renderChecklist({ items: openItems(checklist) })}\n` +
    'Finish each one, then call update_checklist to mark it done with evidence (file and symbol or test), ' +
    'or blocked with the reason. Then summarize.';
}

export function checklistTable(checklist) {
  const cell = (text) => String(text || '-').replaceAll('|', '\\|');
  return '| # | Check | Status | Evidence |\n|---|---|---|---|\n' +
    checklist.items.map(({ id, check, status, evidence }) =>
      `| ${id} | ${cell(check)} | ${status} | ${cell(evidence)} |`).join('\n') + '\n';
}
