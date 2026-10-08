import { createHash } from 'node:crypto';
import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { excellenceFailed, joinLearning, learningSeat, loadLearning, matchesEvaluation } from './learn.mjs';
import { ensureLocalPath } from './paths.mjs';
import { redactSecrets } from '../runtime/memory.mjs';

function signature(reason) {
  const prefix = /^([A-Za-z][A-Za-z /-]{1,40}):\s*/.exec(reason);
  const gate = prefix?.[1].toLowerCase().replace(/\s+/g, '-') ?? 'review';
  const normalized = reason.slice(prefix?.[0].length ?? 0)
    .replace(/`[^`]+`|'[^']+'|"[^"]+"/g, '<identifier>')
    .replace(/(?:[A-Za-z]:[\\/]|\.{0,2}[\\/])?[A-Za-z0-9_@.-]+(?:[\\/][A-Za-z0-9_@.-]+)+/g, '<path>')
    .replace(/\b[\w.-]+\.(?:[cm]?[jt]sx?|md|ya?ml|json)\b/gi, '<path>')
    .replace(/\b(adds|exports|extend|role of)\s+[$A-Za-z_][$\w]*/g, '$1 <identifier>')
    .replace(/\b[a-z]+(?:[A-Z][A-Za-z0-9]*)+\b|\b[a-z]+(?:_[a-z0-9]+)+\b/g, '<identifier>')
    .replace(/\b\d+(?:\.\d+)?\b/g, '<number>')
    .toLowerCase().replace(/\s+/g, ' ').trim();
  return { gate, normalized };
}

export function failureSignatures(records) {
  if (!Array.isArray(records)) throw new TypeError('Failure signatures require a record array');
  const groups = new Map();
  for (const record of records) {
    if (!record || typeof record !== 'object' || Array.isArray(record)) throw new TypeError('Failure history requires object records');
    const evaluation = record.evaluation;
    const failed = excellenceFailed(record) || ['reject', 'rework'].includes(evaluation?.verdict);
    if (!failed) continue;
    const reasons = [...(record.defects ?? []), ...(evaluation?.defects ?? [])];
    if (['reject', 'rework'].includes(evaluation?.verdict) && evaluation.comment?.trim()) {
      reasons.push(evaluation.comment);
    }
    const seat = learningSeat(record);
    const identity = record.task ? `task:${record.task}` :
      record.session ? `session:${record.session}` : record.sha ? `sha:${record.sha.toLowerCase()}` : null;
    if (!identity) continue;
    for (const reason of reasons) {
      if (typeof reason !== 'string' || !reason.trim()) continue;
      const { gate, normalized } = signature(reason);
      if (!normalized) continue;
      const key = `${gate}:${normalized}`;
      if (!groups.has(key)) groups.set(key, { gate, reason: normalized, occurrences: new Map() });
      const group = groups.get(key);
      const occurrence = `${identity}:${seat}`;
      if (!group.occurrences.has(occurrence)) group.occurrences.set(occurrence, {
        issue: /^issue-([1-9]\d*)$/.exec(record.task ?? '')?.[1] ?? null,
        task: record.task ?? null, session: record.session ?? null,
        model: record.model ?? 'unknown', seat,
      });
    }
  }
  return [...groups.entries()].map(([key, group]) => ({
    gate: group.gate, reason: group.reason,
    slug: `${group.gate.replace(/[^a-z0-9-]/g, '-').slice(0, 40)}-${createHash('sha256').update(key).digest('hex').slice(0, 16)}`,
    target: /policy|scope|secret|permission/.test(group.gate + ' ' + group.reason) ? 'principal' : 'skill',
    occurrences: [...group.occurrences.values()].sort((a, b) =>
      `${a.issue ?? a.task ?? a.session}:${a.seat}`.localeCompare(`${b.issue ?? b.task ?? b.session}:${b.seat}`, undefined, { numeric: true })),
  })).sort((a, b) => a.slug.localeCompare(b.slug));
}

function safeText(value, options) {
  return redactSecrets(String(value ?? 'unknown'), options)
    .replace(/[\x00-\x1f\x7f]/g, ' ').replace(/[\\`*_{}[\]<>#|]/g, '\\$&');
}

export function renderFailureProposal(proposal, options = {}) {
  const text = (value) => safeText(value, options);
  return `# Proposed ${proposal.target}: ${text(proposal.gate)}\n\nStatus: draft; human review required.\n` +
    `Target: ${proposal.target}\nOccurrences: ${proposal.occurrences.length} distinct slice/seat identities.\n\n` +
    `## Failure signature\n\n${text(proposal.gate)}: ${text(proposal.reason)}\n\n` +
    `## Evidence\n\n${proposal.occurrences.map((entry) =>
      `- Issue: ${entry.issue ? `#${entry.issue}` : 'unknown'}; task: ${text(entry.task)}; ` +
      `session: ${text(entry.session)}; model: ${text(entry.model)}; seat: ${text(entry.seat)}`).join('\n')}\n\n` +
    '## Proposed rule\n\n' +
    `Before completing this seat, verify that the ${text(proposal.gate)} gate has evidence resolving ` +
    `the recurring condition "${text(proposal.reason)}". If it remains unmet, report the concrete blocker ` +
    'and retain the existing scope and policy boundaries rather than claiming success.\n\n' +
    '## Human promotion\n\nReview the cited outcomes, revise this draft into an actionable rule, and choose the appropriate ' +
    'repository skill packet or principal note. Promotion is a human-owned move/edit; this command never ' +
    'installs a rule, grants capabilities, modifies routing, or changes active skills/principals. ' +
    'After promotion or rejection, move this draft out of the proposals directory.\n';
}

async function proposalDirectory(cwd, create) {
  const directory = path.join(cwd, '.roster', 'proposals');
  await ensureLocalPath(directory, cwd);
  if (create) await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  await ensureLocalPath(directory, cwd);
  return directory;
}

export async function listFailureProposals({ cwd = process.cwd() } = {}) {
  const directory = await proposalDirectory(cwd, false);
  const entries = await fs.readdir(directory, { withFileTypes: true }).catch((error) => {
    if (error.code === 'ENOENT') return [];
    throw error;
  });
  const names = [];
  for (const entry of entries.filter((entry) => entry.name.endsWith('.md'))) {
    if (!entry.isFile() || entry.isSymbolicLink()) throw new Error('Proposal drafts must be regular files');
    if (!/^[a-z0-9-]+-[a-f0-9]{16}\.md$/.test(entry.name)) throw new Error('Proposal draft names must use the generated signature slug');
    const target = path.join(directory, entry.name);
    await ensureLocalPath(target, cwd);
    if ((await fs.lstat(target)).nlink !== 1) throw new Error('Proposal drafts must be single-link files');
    names.push(entry.name);
  }
  return names.sort();
}

export function formatFailureProposals(names) {
  return names.length ? `Open improvement proposals (${names.length}):\n${names.map((name) => `- ${name}`).join('\n')}\n` : '';
}

export async function proposeRecurringFailures({ cwd = process.cwd(), env = process.env, apiKeyEnv } = {}) {
  await ensureLocalPath(path.join(cwd, '.roster', 'runs'), cwd);
  await ensureLocalPath(path.join(cwd, '.roster', 'evals.jsonl'), cwd);
  const { runs, evaluations } = loadLearning({ cwd });
  const records = joinLearning([], runs, evaluations);
  // Unmatched human evaluations without model/class still retain usable failure evidence.
  for (const evaluation of evaluations) {
    if (!records.some((record) => matchesEvaluation(record, evaluation))) {
      const later = evaluations.findLast((entry) => matchesEvaluation(entry, evaluation));
      if (later === evaluation) records.push({ ...evaluation, evaluation });
    }
  }
  const recurring = failureSignatures(records).filter(({ occurrences }) => occurrences.length >= 3);
  if (!recurring.length) return { created: [], existing: [], signatures: [] };
  const directory = await proposalDirectory(cwd, true);
  const created = [];
  const existing = [];
  for (const proposal of recurring) {
    const name = `${proposal.slug}.md`;
    const file = path.join(directory, name);
    await ensureLocalPath(file, cwd);
    let handle;
    try {
      handle = await fs.open(file, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL |
        (constants.O_NOFOLLOW ?? 0), 0o600);
    } catch (error) {
      if (error.code !== 'EEXIST') throw error;
      const entry = await fs.lstat(file);
      if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1) throw new Error('Existing proposal must be a regular single-link draft');
      existing.push(name);
      continue;
    }
    try {
      const owned = await handle.stat();
      try {
        await handle.writeFile(renderFailureProposal(proposal, { env, apiKeyEnv }), 'utf8');
      } catch (error) {
        const current = await fs.lstat(file);
        if (current.dev === owned.dev && current.ino === owned.ino && current.nlink === 1) {
          await fs.unlink(file);
        } else {
          throw new Error('Failed proposal write; draft path changed before cleanup', { cause: error });
        }
        throw error;
      }
    } finally {
      await handle.close();
    }
    created.push(name);
  }
  return { created, existing, signatures: recurring };
}
