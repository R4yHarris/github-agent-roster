// Start-of-run base selection (FEATURE_SPEC sections 3, 4.1, 5.4, 7): a task branch is a claim,
// so new work starts from a freshly fetched trunk unless the user explicitly chooses otherwise.
// Git and GitHub stay the source of truth; nothing here writes state.

export const START_BASES = Object.freeze(['trunk', 'current']);
export const START_SYNCS = Object.freeze(['fetch', 'offline']);
const REF = /^(?!-)(?!.*\.\.)(?!.*\/\/)(?!.*@\{)[A-Za-z0-9._/-]{1,200}(?<![./])$/;

export function validateStartBase(value, name = 'base') {
  if (typeof value !== 'string' || (!START_BASES.includes(value) && !REF.test(value))) {
    throw new TypeError(`${name} must be trunk, current, or a Git ref name`);
  }
  return value;
}

export function startOptions(config = {}, overrides = {}) {
  const base = validateStartBase(overrides.base ?? config.start?.base ?? 'trunk');
  const sync = overrides.sync ?? config.start?.sync ?? 'fetch';
  if (!START_SYNCS.includes(sync)) throw new TypeError('sync must be fetch or offline');
  return { base, sync };
}

function firstLine(error) {
  return String(error?.stderr || error?.message || error).trim().split(/\r?\n/)[0].slice(0, 200);
}

async function attempt(runCommand, args, repoRoot) {
  try {
    return { ok: true, out: String(await runCommand('git', args, repoRoot) ?? '').trim() };
  } catch (error) {
    return { ok: false, error };
  }
}

async function trunkCandidates(runCommand, repoRoot) {
  const candidates = [];
  const head = await attempt(runCommand, ['symbolic-ref', '--quiet', '--short', 'refs/remotes/origin/HEAD'], repoRoot);
  if (head.ok && /^origin\/[^\s]+$/.test(head.out)) candidates.push(head.out);
  const known = await attempt(runCommand,
    ['for-each-ref', '--format=%(refname:short)', 'refs/remotes/origin/main', 'refs/remotes/origin/master'], repoRoot);
  const names = known.ok ? known.out.split(/\r?\n/).filter((name) => /^origin\/(main|master)$/.test(name)) : [];
  for (const name of ['origin/main', 'origin/master']) {
    if (names.includes(name) && !candidates.includes(name)) candidates.push(name);
  }
  return candidates;
}

async function commitOf(runCommand, ref, repoRoot) {
  const result = await attempt(runCommand, ['rev-parse', '--verify', '--quiet', '--end-of-options', `${ref}^{commit}`], repoRoot);
  return result.ok && /^[0-9a-f]{40,64}$/.test(result.out) ? result.out : null;
}

/**
 * Fetch origin (unless offline) and choose the start point for a new task branch.
 * Returns { mode, ref, sha, fetched, notes } where ref is the start point to pass to
 * `git worktree add -b`, or null to start from the caller's HEAD.
 */
export async function resolveStart({ repoRoot, runCommand, base = 'trunk', sync = 'fetch' }) {
  validateStartBase(base);
  if (!START_SYNCS.includes(sync)) throw new TypeError('sync must be fetch or offline');
  const notes = [];
  const origin = await attempt(runCommand, ['remote', 'get-url', 'origin'], repoRoot);
  let fetched = 'skipped';
  if (!origin.ok || !origin.out) {
    notes.push('no origin remote; freshness unknown');
  } else if (sync === 'offline') {
    notes.push('fetch skipped (offline); freshness unknown');
  } else {
    const fetch = await attempt(runCommand, ['fetch', '--prune', '--quiet', 'origin'], repoRoot);
    fetched = fetch.ok;
    if (!fetch.ok) notes.push(`fetch failed (${firstLine(fetch.error)}); freshness unknown`);
  }

  if (base === 'current') return { mode: 'current', ref: null, sha: null, fetched, notes };

  if (base !== 'trunk') {
    const sha = await commitOf(runCommand, base, repoRoot);
    if (!sha) throw new Error(`Start base ${base} does not name a commit; fetch it or choose --base trunk`);
    return { mode: 'ref', ref: base, sha, fetched, notes };
  }

  const candidates = origin.ok && origin.out ? await trunkCandidates(runCommand, repoRoot) : [];
  for (const trunk of candidates) {
    const sha = await commitOf(runCommand, trunk, repoRoot);
    if (sha) return { mode: 'trunk', ref: trunk, sha, fetched, notes };
  }
  notes.push('origin default branch not found; starting from current HEAD');
  return { mode: 'current', ref: null, sha: null, fetched, notes };
}

/** Count how far an existing branch is behind/ahead of the start ref. Null when unknown. */
export async function branchDrift({ repoRoot, runCommand, branch, start }) {
  if (!start?.ref) return null;
  const result = await attempt(runCommand,
    ['rev-list', '--left-right', '--count', `refs/heads/${branch}...${start.ref}`], repoRoot);
  const match = result.ok && /^(\d+)\s+(\d+)$/.exec(result.out);
  return match ? { ahead: Number(match[1]), behind: Number(match[2]) } : null;
}

export function formatStart(start, drift = null) {
  if (!start) return 'Start: unknown';
  const where = start.mode === 'current' ? 'current HEAD' : `${start.mode} ${start.ref}@${start.sha.slice(0, 7)}`;
  const freshness = start.fetched === true ? 'fetched' : 'freshness unknown';
  const parts = [`Start: ${where} (${freshness})`];
  if (drift?.behind) parts.push(`existing branch is ${drift.behind} behind ${start.ref}; not moved`);
  return [...parts, ...(start.notes ?? [])].join('; ');
}
