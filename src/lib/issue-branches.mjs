// Task IDs and directory names stay stable across the branch namespace migration.
export function issueTask(number) {
  if (!/^[1-9]\d*$/.test(String(number)) || !Number.isSafeInteger(Number(number))) {
    throw new TypeError('Issue number must be a positive safe integer');
  }
  return `issue-${number}`;
}

export function issueBranch(number, attempt) {
  issueTask(number);
  if (attempt !== undefined && (!Number.isInteger(attempt) || attempt < 1 || attempt > 16)) {
    throw new TypeError('Attempt index must be between 1 and 16');
  }
  return `agents/${number}${attempt === undefined ? '' : `-a${attempt}`}`;
}

export function issueBranches(number, attempt) {
  const canonical = issueBranch(number, attempt);
  return [canonical, `${issueTask(number)}${attempt === undefined ? '' : `-a${attempt}`}`];
}

export function parseIssueBranch(branch) {
  const match = /^(?:agents\/|issue-)([1-9]\d*)(?:-a([1-9]|1[0-6]))?$/.exec(branch ?? '');
  if (!match || !Number.isSafeInteger(Number(match[1]))) return null;
  return { number: Number(match[1]), attempt: match[2] ? Number(match[2]) : null };
}

export function parseWorktreeInventory(inventory) {
  return inventory.split('\0\0').filter(Boolean).map((record) => {
    const fields = record.split('\0');
    return {
      path: fields.find((field) => field.startsWith('worktree '))?.slice(9),
      branch: fields.find((field) => field.startsWith('branch refs/heads/'))?.slice(18) ?? null,
    };
  });
}

export function selectIssueBranch(number, branches, attempt) {
  const candidates = issueBranches(number, attempt);
  const found = candidates.filter((branch) => branches.includes(branch));
  if (found.length > 1) {
    throw new Error(`Ambiguous issue branch namespaces for #${number}; preserve both branches and resolve ownership explicitly`);
  }
  return found[0] ?? candidates[0];
}

export async function resolveIssueBranch({ number, repoRoot, runCommand, worktrees, attempt }) {
  const candidates = issueBranches(number, attempt);
  const inventory = worktrees ?? parseWorktreeInventory(await runCommand('git',
    ['worktree', 'list', '--porcelain', '-z'], repoRoot));
  const refs = (await runCommand('git', ['for-each-ref', '--format=%(refname)',
    ...candidates.map((branch) => `refs/heads/${branch}`)], repoRoot)).trim().split(/\r?\n/)
    .filter((ref) => candidates.some((branch) => ref === `refs/heads/${branch}`))
    .map((ref) => ref.slice(11));
  const branch = selectIssueBranch(number, [...refs, ...inventory.map((entry) => entry.branch)], attempt);
  const registered = inventory.filter((entry) => entry.branch === branch);
  if (registered.length > 1) throw new Error('Two issues cannot share a worktree branch; isolation is refused.');
  return { branch, known: refs.includes(branch), worktree: registered[0] ?? null, worktrees: inventory };
}
