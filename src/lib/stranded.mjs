export const defaultStaleThresholdMs = 30 * 60 * 1000;

function timestamp(value, name) {
  const time = value instanceof Date ? value.getTime()
    : typeof value === 'string' ? Date.parse(value) : value;
  if (typeof time !== 'number' || !Number.isFinite(time)) {
    throw new TypeError(`${name} must be a valid timestamp`);
  }
  return time;
}

export function classifyStrandedWork({
  issues, worktrees = [], branches = [], heartbeats = [], now,
  thresholdMs = defaultStaleThresholdMs,
} = {}) {
  if (![issues, worktrees, branches, heartbeats].every(Array.isArray)) {
    throw new TypeError('Stranded-work detection requires issue, worktree, branch, and heartbeat arrays');
  }
  const current = timestamp(now, 'now');
  if (!Number.isSafeInteger(thresholdMs) || thresholdMs <= 0) {
    throw new TypeError('Heartbeat threshold must be a positive safe integer in milliseconds');
  }
  const claims = new Set(branches);
  for (const worktree of worktrees) {
    if (typeof worktree?.branch !== 'string') throw new TypeError('Worktree must name its branch');
    claims.add(worktree.branch);
  }
  if ([...claims].some((branch) => typeof branch !== 'string' || !branch)) {
    throw new TypeError('Branches must be nonempty strings');
  }
  const latest = new Map();
  for (const heartbeat of heartbeats) {
    if (!Number.isSafeInteger(heartbeat?.issue) || heartbeat.issue <= 0) {
      throw new TypeError('Heartbeat must name a positive safe issue number');
    }
    const time = timestamp(heartbeat.timestamp, 'Heartbeat');
    latest.set(heartbeat.issue, Math.max(latest.get(heartbeat.issue) ?? -Infinity, time));
  }
  const results = [];
  for (const issue of issues) {
    if (!Number.isSafeInteger(issue?.number) || issue.number <= 0 ||
        !['OPEN', 'CLOSED', 'open', 'closed'].includes(issue.state) || !Array.isArray(issue.assignees)) {
      throw new TypeError('Issue must have a positive safe number, open/closed state, and assignees');
    }
    if (issue.state.toLowerCase() !== 'open' || !issue.assignees.length) continue;
    const branch = `issue-${issue.number}`;
    const lastHeartbeat = latest.get(issue.number) ?? null;
    const status = !claims.has(branch) ? 'stranded'
      : lastHeartbeat === null || current - lastHeartbeat > thresholdMs ? 'stale' : 'healthy';
    results.push({ issue, branch, status, lastHeartbeat });
  }
  return results;
}
