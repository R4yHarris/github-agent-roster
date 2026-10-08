import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { ensureLocalPath } from './paths.mjs';
import { createRunLog } from './run-log.mjs';
import { recordRun } from './learn.mjs';
import { buildRun } from '../metrics/run.mjs';
import { readPlannerTask, readPreviousReview } from '../seats/planner.mjs';
import { checkExcellence } from '../runtime/excellence.mjs';
import { throwIfCancelled } from '../runtime/cancel.mjs';

const execute = promisify(execFile);

export async function runOnlyReview(run, {
  repoRoot, config, env = process.env, again = false, fetchImpl, vault, signal, onRunEvent,
  debug, errorOutput = process.stderr,
}) {
  if (!run?.worktreePath || !run.result?.resultPath) throw new Error('A completed coder result is required; run or resume a task before /review.');
  if (run.planningOnly) throw new Error('Planning-only output is not a completed coder result to review.');
  throwIfCancelled(signal);
  const worktree = run.worktreePath;
  const task = await readPlannerTask(worktree);
  if (task === null) throw new Error('Review requires TASK.md in the current worktree.');
  const previous = await readPreviousReview(worktree);
  if (previous !== null) {
    if (!again) throw new Error('REVIEW.md already exists; use /review --again to rerun the current diff.');
    const tracked = (await execute('git', ['ls-files', '--', 'REVIEW.md'], {
      cwd: worktree, encoding: 'utf8', timeout: 30000,
    })).stdout;
    if (tracked.trim()) throw new Error('A tracked REVIEW.md cannot be replaced by the reviewer.');
    const common = path.resolve(worktree, (await execute('git', ['rev-parse', '--git-common-dir'], {
      cwd: worktree, encoding: 'utf8', timeout: 30000,
    })).stdout.trim());
    const archive = path.join(common, 'roster-reviews', `${Date.now()}-${randomBytes(8).toString('hex')}`, 'REVIEW.md');
    await ensureLocalPath(archive, common);
    await fs.mkdir(path.dirname(archive), { recursive: true, mode: 0o700 });
    await fs.rename(path.join(worktree, 'REVIEW.md'), archive);
  }
  const excellence = await checkExcellence({ worktree, task, result: run.result,
    baseline: run.result.baseline, verifiedSnapshot: run.result.excellence?.snapshot, memoryPath: run.result.memoryPath,
    env, apiKeyEnv: config.llm.api_key_env });
  const coderResult = { ...run.result, excellence };
  const session = run.sessions?.reviewer ?? `roster-${randomBytes(8).toString('hex')}-reviewer`;
  const logger = await createRunLog({ repoRoot: run.repoRoot ?? worktree, session, env, debug,
    issue: run.issue?.number ?? null, errorOutput, observe: onRunEvent });
  const review = await logger.seat('reviewer', session, config, async (onEvent) => (await import('../seats/reviewer.mjs')).runReviewer({
    worktree, repoRoot, config, coderResult, env, fetchImpl, vault, signal, onEvent, askKind: run.askKind ?? 'slice',
  }));
  const measured = review.response ? buildRun({ config, response: review.response, task: run.task, session, env: {} }) : null;
  if (path.relative(path.resolve(run.repoRoot ?? worktree), path.resolve(worktree))) {
    await recordRun({ task: run.task, session, task_class: run.planner?.metadata?.task_class,
      seat: 'reviewer', delivery: logger.delivery('reviewer'),
      provider: measured?.provider }, { cwd: run.repoRoot, env: measured?.env ?? {}, run: measured, createDirectory: true });
  }
  return { ...run, result: coderResult, review, sessions: { ...run.sessions, reviewer: session },
    delivery: { ...run.delivery, reviewer: logger.delivery('reviewer') },
    runs: { ...run.runs, reviewer: measured } };
}
