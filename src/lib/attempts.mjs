import { randomBytes } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { ensureLocalPath } from './paths.mjs';
import { initializeWorktreeSubmodules } from './contracts.mjs';
import { recordRun } from './learn.mjs';
import { isManagedFile } from '../runtime/tools.mjs';
import { throwIfCancelled } from '../runtime/cancel.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';
import { renderAssignment } from '../planner/stub.mjs';

export function attemptLimit(value = 1, maximum = 3) {
  if (!Number.isSafeInteger(maximum) || maximum < 1 || maximum > 16 ||
      !Number.isSafeInteger(value) || value < 1 || value > maximum) {
    throw new TypeError(`--attempts must be an integer from 1 to seat.max_attempts (${maximum}); maximum is 16`);
  }
  return value;
}

export function attemptGates(result) {
  return {
    tests: result?.testsSkipped ? 'skipped' : result?.tests?.exit_code === 0 ? 'pass' : 'fail',
    excellence: result?.excellence?.pass ? 'pass' : 'fail',
    red_green: result?.redGreen?.status === 'unavailable' ? 'fail'
      : !result?.redGreen || result.redGreen.status !== 'checked' ? 'skipped'
      : result.redGreen.notRed?.length ? 'fail' : 'pass',
    shadow: result?.shadow?.status === 'unavailable' ? 'fail'
      : !result?.shadow || ['skipped', 'none'].includes(result.shadow.status) ? 'skipped'
      : result.shadow.findings?.length ? 'fail' : 'pass',
  };
}

export function selectAttempt(attempts) {
  return attempts.filter((attempt) => !attempt.error && !attempt.completed?.failed && attempt.completed?.result?.mode === 'llm' &&
    Object.values(attempt.evidence.gates).every((gate) => gate !== 'fail') &&
    attempt.evidence.review === 'pass')
    .sort((left, right) => left.evidence.changed_lines - right.evidence.changed_lines ||
      left.evidence.duration_ms - right.evidence.duration_ms || left.evidence.index - right.evidence.index)[0] ?? null;
}

export function attemptSummary(run) {
  return `\n\n## Attempts\n\nSelected attempt ${run.winner} of ${run.attempts}; only the winner is published.\n\n` +
    run.attemptResults.map((attempt) => `- ${attempt.index}: profile=${attempt.profile}; ` +
      `hardware=${attempt.hardware}; gates=${Object.entries(attempt.gates).map(([gate, verdict]) =>
        `${gate}:${verdict}`).join(',')}; review=${attempt.review}; changed_lines=${attempt.changed_lines}; ` +
      `duration_ms=${attempt.duration_ms}`).join('\n');
}

async function changedLines(worktree, command) {
  const stat = await command('git', ['diff', '--numstat', 'HEAD'], worktree);
  let lines = stat.split('\n').filter(Boolean).reduce((total, line) => {
    const [added, removed] = line.split('\t');
    return total + (added === '-' ? 1 : Number(added)) + (removed === '-' ? 1 : Number(removed));
  }, 0);
  const untracked = (await command('git', ['ls-files', '--others', '--exclude-standard', '-z'], worktree))
    .split('\0').filter((file) => file && !isManagedFile(file));
  for (const file of untracked) {
    const target = path.join(worktree, file);
    await ensureLocalPath(target, worktree);
    const entry = await fs.lstat(target);
    if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1) {
      throw new Error('Attempt diff requires regular single-link files');
    }
    const text = await fs.readFile(target, 'utf8');
    lines += text.split('\n').length - Number(text.endsWith('\n'));
  }
  if (!Number.isSafeInteger(lines) || lines < 0) throw new Error('Attempt diff returned invalid line counts');
  return lines;
}

export async function runPlanAttempts({
  count, prepared, planner, config, env, signal, command, cleanupCommand = command, choose, execute, publish, log,
}) {
  if (attemptLimit(count, config.seat.max_attempts ?? 3) < 2) throw new TypeError('Attempt execution requires at least two candidates');
  if (prepared.local || prepared.issue?.number == null) throw new TypeError('Attempts require a GitHub issue slice');
  const status = await command('git', ['status', '--porcelain=v1', '--no-renames', '-z', '--untracked-files=all'],
    prepared.worktreePath);
  if (status.split('\0').filter(Boolean).some((entry) => !isManagedFile(entry.slice(3)))) {
    throw new Error('Multiple attempts require a clean application baseline; preserve existing edits and use --attempts 1');
  }
  const routes = [];
  for (let index = 0; index < count; index += 1) {
    const selected = await choose(routes.map(({ profile }) => profile.id));
    if (!selected || routes.some(({ profile }) => profile.id === selected.profile.id)) {
      throw new Error(`--attempts ${count} requires ${count} distinct eligible fleet profiles`);
    }
    routes.push(selected);
  }
  const base = (await command('git', ['rev-parse', 'HEAD'], prepared.worktreePath)).trim();
  const attempts = [];
  const created = [];
  const batch = randomBytes(8).toString('hex');
  const record = async (attempt) => {
    const result = attempt.completed?.result ?? attempt.error.result;
    await recordRun({ session: result.run?.metrics.session ?? attempt.session,
      task: prepared.task, seat: 'coder', task_class: planner.metadata.task_class,
      excellence: result.excellence?.pass ? 'pass' : 'fail', attempt: attempt.evidence },
    { cwd: prepared.repoRoot, env: {}, createDirectory: true, run: result.run });
  };
  const handoff = { 'TASK.md': planner.task, 'RECIPE.yml': planner.recipe, 'ESTIMATE.md': planner.estimate,
    'ASSIGNMENT.md': renderAssignment(prepared.issue) };
  for (const [name, content] of Object.entries(handoff)) {
    const target = path.join(prepared.worktreePath, name);
    await ensureLocalPath(target, prepared.worktreePath);
    const entry = await fs.lstat(target);
    if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1 || entry.size > 65536 ||
        (await fs.readFile(target, 'utf8')).replaceAll('\r\n', '\n') !== content.replaceAll('\r\n', '\n')) {
      throw new Error('Attempt handoff changed after planning');
    }
  }
  const remove = async (attempt) => {
    await cleanupCommand('git', ['worktree', 'remove', '--force', attempt.worktreePath], prepared.repoRoot);
    await cleanupCommand('git', ['branch', '-D', attempt.branch], prepared.repoRoot);
  };
  try {
    for (let index = 1; index <= count; index += 1) {
      throwIfCancelled(signal);
      const branch = `issue-${prepared.issue.number}-a${index}`;
      const worktreePath = path.join(path.dirname(prepared.worktreePath), branch);
      await ensureLocalPath(worktreePath, prepared.repoRoot);
      const entry = await fs.lstat(worktreePath).catch((error) => {
        if (error.code === 'ENOENT') return null;
        throw error;
      });
      if (entry) throw new Error('Attempt worktree path already exists; refusing to adopt or overwrite it');
      await command('git', ['worktree', 'add', '-b', branch, worktreePath, base], prepared.repoRoot);
      const attempt = { branch, worktreePath, selected: routes[index - 1] };
      created.push(attempt);
      await initializeWorktreeSubmodules(worktreePath, command);
      for (const [name, content] of Object.entries(handoff)) {
        const target = path.join(worktreePath, name);
        await ensureLocalPath(target, worktreePath);
        await fs.writeFile(target, content, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
      }
      const session = `roster-${prepared.issue.number}-a${index}-${randomBytes(4).toString('hex')}-coder`;
      attempt.session = session;
      const attemptPrepared = { ...prepared, worktreePath, session, reused: false,
        assignmentPath: path.join(worktreePath, 'ASSIGNMENT.md') };
      const attemptPlanner = { ...planner, reused: false, critic: undefined,
        taskPath: path.join(worktreePath, 'TASK.md'), recipePath: path.join(worktreePath, 'RECIPE.yml'),
        estimatePath: path.join(worktreePath, 'ESTIMATE.md') };
      log(`Attempt ${index}/${count}: profile=${attempt.selected.profile.id} worktree=${worktreePath}`);
      const started = performance.now();
      try {
        attempt.completed = await execute(attemptPrepared, attemptPlanner, attempt.selected);
      } catch (error) {
        throwIfCancelled(signal);
        if (!(error instanceof Error) || !error.result) throw error;
        attempt.error = error;
      }
      const result = attempt.completed?.result ?? attempt.error.result;
      attempt.evidence = {
        batch, count, index, profile: attempt.selected.profile.id,
        hardware: redactEvidence(attempt.selected.profile.hardware || 'unknown',
          { env, apiKeyEnv: config.llm.api_key_env }),
        gates: attemptGates(result), review: attempt.completed?.review?.verdict ?? 'unavailable',
        changed_lines: await changedLines(worktreePath, command),
        duration_ms: Math.round(performance.now() - started), winner: 0,
      };
      attempts.push(attempt);
    }
  } catch (error) {
    for (const attempt of attempts) await record(attempt);
    // Only paths created by this invocation may be removed. Preserve the failing tree for diagnosis.
    for (const attempt of created.slice(0, -1)) await remove(attempt);
    throw error;
  }
  const winner = selectAttempt(attempts);
  for (const attempt of attempts) {
    attempt.evidence.winner = winner?.evidence.index ?? 0;
    await record(attempt);
  }
  const kept = winner ?? attempts.at(-1);
  for (const attempt of attempts.filter((attempt) => attempt !== kept)) await remove(attempt);
  const evidence = attempts.map(({ evidence }) => evidence);
  log(winner ? `Best of ${count}: attempt ${winner.evidence.index} passed all gates and review.`
    : `Best of ${count}: no attempt passed all gates and review; no publication.`);
  if (!winner && kept.error) {
    kept.error.attemptResults = evidence;
    throw kept.error;
  }
  const completed = { ...kept.completed, attempt: kept.evidence, attempts: count, winner: winner?.evidence.index ?? null,
    attemptResults: evidence, failed: !winner,
    run: kept.completed.run ? { ...kept.completed.run, attempts: count, winner: winner?.evidence.index ?? null,
      losers: evidence.filter(({ index }) => index !== winner?.evidence.index) } : null };
  if (winner && publish) await publish(completed, winner.selected);
  return completed;
}
