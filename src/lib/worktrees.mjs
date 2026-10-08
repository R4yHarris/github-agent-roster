import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { repositoryRoot } from './learn.mjs';
import { registeredWorktrees } from './local-runs.mjs';
import { readIssueLogs } from './run-log.mjs';

const execute = promisify(execFile);

export function assertIsolatedIssueBranches(worktrees) {
  const branches = worktrees.filter((entry) => /^issue-[1-9]\d*(?:-a(?:[1-9]|1[0-6]))?$/.test(entry.branch ?? '')).map(({ branch }) => branch);
  if (new Set(branches).size !== branches.length) {
    throw new Error('Two issues cannot share a worktree branch; isolation is refused.');
  }
}

export async function listIssueWorktrees({ cwd = process.cwd(), env = process.env }) {
  const root = repositoryRoot(cwd);
  const inventory = await registeredWorktrees(root);
  assertIsolatedIssueBranches(inventory);
  const results = [];
  for (const entry of inventory.filter((item) => /^issue-[1-9]\d*(?:-a(?:[1-9]|1[0-6]))?$/.test(item.branch ?? ''))) {
    if (typeof entry.path !== 'string') throw new Error('Registered issue worktree has no path');
    const number = Number(/^issue-([1-9]\d*)/.exec(entry.branch)[1]);
    const logs = await readIssueLogs({ repoRoot: root, issue: number, env });
    const last = logs.sort((left, right) => left.lastLine.localeCompare(right.lastLine)).at(-1);
    let status;
    try {
      ({ stdout: status } = await execute('git', ['status', '--porcelain=v1', '-z', '--untracked-files=normal'], {
        cwd: entry.path, env, encoding: 'utf8', timeout: 30000, maxBuffer: 4 * 1024 * 1024,
      }));
    } catch (error) {
      throw new Error('Could not inspect registered issue worktree cleanliness.', { cause: error });
    }
    results.push({ path: entry.path, branch: entry.branch, seat: last?.lastSeat ?? '-',
      status: status ? 'dirty' : 'clean' });
  }
  return results.sort((left, right) => left.branch.localeCompare(right.branch, undefined, { numeric: true }));
}
