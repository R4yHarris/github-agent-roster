import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify, stripVTControlCharacters } from 'node:util';
import { parseIssueBody, validateIssueNumber } from './issue.mjs';
import { repositoryRoot } from './learn.mjs';
import { ensureLocalPath } from './paths.mjs';
import { createProvenanceStore, instrumentLifecycle } from './provenance-api.mjs';
import { readStatus } from './status.mjs';
import { readPlannerTask, readPreviousReview } from '../seats/planner.mjs';
import { parseTaskDocument } from '../planner/task.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';
import { classifyAsk } from '../planner/classify.mjs';
import { validatedPlanTask } from '../planner/plan-mode.mjs';

const execute = promisify(execFile);
const samePath = (left, right) => process.platform === 'win32'
  ? path.resolve(left).toLowerCase() === path.resolve(right).toLowerCase() : path.resolve(left) === path.resolve(right);

/**
 * Opt-out environment flag: when truthy, run/session lifecycle events are
 * captured in memory (so the lifecycle still completes and callers observe the
 * same event order) but no durable provenance record is written.
 */
export function provenanceOptOut(env = process.env) {
  const value = env?.ROSTER_PROVENANCE_OPT_OUT ?? env?.ROSTER_PROVENANCE_DISABLED;
  if (typeof value === 'string') return /^(1|true|yes|on)$/i.test(value.trim());
  return value === true;
}

/**
 * Build the typed provenance store for a local run. Identity comes from
 * repo-identity.mjs (git common dir + origin remote hash), never from a raw
 * path, so two repositories cannot collide and records cannot leak across
 * repositories. Returns null when identity cannot be derived; provenance is
 * best-effort and must never break the run lifecycle.
 */
export function provenanceStoreForRun({ repoRoot, env = process.env, run } = {}) {
  if (!repoRoot) return null;
  try {
    return createProvenanceStore({
      root: path.join(repoRoot, '.git', 'roster', 'provenance'),
      repoRoot,
      run,
    });
  } catch {
    return null;
  }
}

/**
 * Capture a run/session lifecycle event through the typed API. Resolves to a
 * non-durable marker when provenance is opted out or unavailable, so the
 * lifecycle still completes either way.
 */
export async function captureLifecycleEvent(store, { event, runId, sessionId, payload, optOut } = {}) {
  if (!store || optOut) {
    return { event, runId, sessionId, durable: false };
  }
  try {
    const record = await store.recordEvent({ runId, sessionId, event, payload });
    return { ...record, durable: true };
  } catch {
    // Provenance capture is best-effort: a failure must never break the run.
    return { event, runId, sessionId, durable: false };
  }
}

/**
 * Run/session lifecycle hooks bound to a local run. Covers all five required
 * lifecycle cases: started, session, failure, cancellation, completed.
 */
export function lifecycleHooks(store, { optOut = provenanceOptOut() } = {}) {
  const base = instrumentLifecycle(store, { optOut });
  const hooks = {};
  for (const event of ['started', 'session', 'failure', 'cancellation', 'completed']) {
    hooks[event] = async (ctx = {}) => captureLifecycleEvent(store, { ...ctx, event, optOut });
  }
  return { ...base, ...hooks };
}

export async function registeredWorktrees(root) {
  const { stdout } = await execute('git', ['worktree', 'list', '--porcelain', '-z'], {
    cwd: root, encoding: 'utf8', timeout: 30000, maxBuffer: 1024 * 1024,
  });
  return stdout.split('\0\0').filter(Boolean).map((record) => {
    const fields = record.split('\0');
    return { path: fields.find((field) => field.startsWith('worktree '))?.slice(9),
      branch: fields.find((field) => field.startsWith('branch refs/heads/'))?.slice(18) ?? null };
  });
}

async function artifact(worktree, name) {
  const file = path.join(worktree, name);
  await ensureLocalPath(file, worktree);
  const entry = await fs.lstat(file).catch((error) => { if (error.code === 'ENOENT') return null; throw error; });
  if (!entry) return null;
  if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1 || entry.size > 65536) {
    throw new Error('Local run artifacts must be bounded regular single-link files');
  }
  return (await fs.readFile(file, 'utf8')).replaceAll('\r\n', '\n');
}

export async function readLocalRun({ number, cwd = process.cwd(), config, env = process.env, root = repositoryRoot(cwd) }) {
  number = validateIssueNumber(number);
  const task = `issue-${number}`;
  const local = await readStatus({ issue: number, offline: true, repoRoot: root, cwd: root, config, env });
  const { worktreePath } = local;
  await ensureLocalPath(worktreePath, root);
  const registered = (await registeredWorktrees(root)).find((entry) => entry.path && samePath(entry.path, worktreePath));
  if (!registered || registered.branch !== local.branch) throw new Error('The requested issue worktree is not registered on its isolated branch');
  const assignment = await artifact(worktreePath, 'ASSIGNMENT.md');
  const match = assignment && /^# Assignment\n\n- Issue URL: (https:\/\/github\.com\/[^\s]+\/issues\/([1-9]\d*))\n- Issue number: ([1-9]\d*)\n- Title: ([^\n]+)\n\n(?:- Wave: ([1-8])\n\n)?## Ask\n\n([\s\S]+)$/.exec(assignment);
  if (!match || Number(match[2]) !== number || Number(match[3]) !== number) throw new Error('Local assignment does not match the requested issue');
  const body = match[6].replace(/\n$/, '');
  const { ask, metadata } = parseIssueBody(body);
  const issue = { number, title: match[4], body, url: match[1], state: 'UNKNOWN',
    ...(match[5] ? { labels: [{ name: `wave:${match[5]}` }] } : {}) };
  const review = await readPreviousReview(worktreePath);
  const result = await artifact(worktreePath, 'RESULT.md');
  const verdict = /^Verdict: (pass|fail)$/m.exec(review ?? '')?.[1] ?? null;
  const state = verdict === 'pass' ? 'passed' : verdict === 'fail' || /Checks: FAIL/.test(result ?? '')
    ? 'failed' : 'idle';
  const askKind = classifyAsk(ask, { title: issue.title }).kind;
  const planMode = askKind === 'slice' && local.artifacts['PLAN.md'] && !local.artifacts['TASK.md'] &&
    !local.artifacts['RECIPE.yml'];
  if (planMode) validatedPlanTask(await artifact(worktreePath, 'PLAN.md'), ask, issue.title);
  return { ...local, issue, ask, metadata, repoRoot: root, worktreePath, task,
    session: local.attempt ? local.lastRun.session : `roster-${number}-coder`,
    assignmentPath: path.join(worktreePath, 'ASSIGNMENT.md'), envPath: path.join(worktreePath, '.env'),
    reused: true, seat: local.runLog?.lastSeat ?? 'coder', state: planMode ? 'planning' : state, reviewVerdict: verdict,
    askKind, ...(planMode ? { planMode: true, planningOnly: true, planPath: path.join(worktreePath, 'PLAN.md') } : {}) };
}

export async function listLocalRuns({ cwd = process.cwd(), config, env = process.env }) {
  const root = repositoryRoot(cwd);
  const runs = [];
  for (const entry of await registeredWorktrees(root)) {
    const match = /^issue-([1-9]\d*)$/.exec(entry.branch ?? '');
    if (match && entry.path && samePath(entry.path, path.join(root, config.paths.worktrees, entry.branch))) {
      runs.push(await readLocalRun({ number: Number(match[1]), root, config, env }));
    }
  }
  return runs.sort((left, right) => left.issue.number - right.issue.number);
}

export async function recapRun(run, { finishReason, env = process.env } = {}) {
  if (!run?.worktreePath) throw new Error('Run or resume a task before /recap.');
  const task = await readPlannerTask(run.worktreePath);
  const document = task === null ? null : parseTaskDocument(task);
  const review = /^Verdict: (pass|fail)$/m.exec(await readPreviousReview(run.worktreePath) ?? '')?.[1] ?? null;
  const result = await artifact(run.worktreePath, 'RESULT.md');
  const exit = /- node --test exited (\d+)/.exec(result ?? '')?.[1];
  const safe = (value) => stripVTControlCharacters(redactEvidence(String(value ?? '-'), { env }))
    .replace(/[\x00-\x1f\x7f]/g, '?');
  return `Outcome: ${safe(document?.title ?? run.issue?.title)} | Files: ${safe(document?.files_allowed.join(', '))}` +
    ` | Last test: ${exit === undefined ? '-' : `node --test (exit ${exit})`}` +
    ` | Review: ${review ?? '-'} | Finish reason: ${safe(finishReason ?? run.result?.finishReason)}\n`;
}
