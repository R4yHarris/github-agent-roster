import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { ensureLocalPath } from '../lib/paths.mjs';
import { taskFilesAllowed } from '../planner/stub.mjs';
import { splitTaskFrontmatter } from './skills.mjs';
import { isAllowedFile, isForbiddenRead, isForbiddenWrite, isManagedFile, isRunLog, isDebugLog, isShellHistory, isCheckpoint, taskAndRepairFiles } from './tools.mjs';

const execute = promisify(execFile);

export function taskSkipsTests(task) {
  const { frontmatter } = splitTaskFrontmatter(task);
  const declarations = frontmatter.split('\n').filter((line) => /^tests:/.test(line));
  if (declarations.length > 1) throw new Error('TASK.md must not repeat tests');
  if (!declarations.length) return false;
  const value = /^tests: (none|required)[ \t]*$/.exec(declarations[0]);
  if (!value) throw new Error('TASK.md tests must be none or required');
  return value[1] === 'none';
}

export function redactEvidence(text, { env = process.env, apiKeyEnv = 'ROSTER_API_KEY' } = {}) {
  let safe = text;
  const values = Object.entries(env).filter(([name, value]) =>
    typeof value === 'string' && value && (name === apiKeyEnv ||
      /TOKEN|PASSWORD|SECRET|PRIVATE_KEY|API_KEY/i.test(name)))
    .map(([, value]) => value).sort((left, right) => right.length - left.length);
  for (const value of values) safe = safe.split(value).join('[redacted]');
  return safe
    .replace(/-----BEGIN [^-\r\n]*PRIVATE KEY-----[\s\S]*?(?:-----END [^-\r\n]*PRIVATE KEY-----|$)/g,
      '[redacted private key]')
    .replace(/\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{24,})\b/g,
      '[redacted credential]');
}

function ignoredNotebook(file, worktree, memoryPath) {
  return isRunLog(file) || isDebugLog(file) || isShellHistory(file) || isCheckpoint(file) || file === '.roster/memory' || file.startsWith('.roster/memory/') ||
    (memoryPath && path.relative(path.resolve(worktree, file), path.resolve(memoryPath)) === '');
}

export async function snapshotWorktree(worktree, { memoryPath } = {}) {
  const root = path.resolve(worktree);
  const snapshot = new Map();
  async function visit(directory = '') {
    for (const entry of await fs.readdir(path.join(root, directory), { withFileTypes: true })) {
      const file = path.posix.join(directory, entry.name);
      if (file === '.git' || file === 'RESULT.md' || file === 'REVIEW.md' ||
          ignoredNotebook(file, root, memoryPath)) continue;
      const target = path.join(root, file);
      const stat = await fs.lstat(target, { bigint: true });
      if (entry.isDirectory() && !entry.isSymbolicLink()) {
        if (isForbiddenRead(file)) {
          snapshot.set(file, `protected:${stat.ino}:${stat.size}:${stat.mtimeNs}:${stat.ctimeNs}`);
        }
        await visit(file);
      } else if (isForbiddenRead(file)) {
        snapshot.set(file, `protected:${stat.ino}:${stat.size}:${stat.mtimeNs}:${stat.ctimeNs}`);
      } else if (entry.isSymbolicLink()) {
        snapshot.set(file, `symlink:${await fs.readlink(target)}`);
      } else if (entry.isFile()) {
        snapshot.set(file, createHash('sha256').update(await fs.readFile(target)).digest('hex'));
      } else snapshot.set(file, `special:${stat.mode}`);
    }
  }
  await visit();
  return snapshot;
}

async function gitChanges(worktree, memoryPath) {
  try {
    await fs.lstat(path.join(worktree, '.git'));
  } catch (error) {
    if (error.code === 'ENOENT') return { files: [], diff: '' };
    throw error;
  }
  const git = async (args) => (await execute('git', args, {
    cwd: worktree, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024, timeout: 60_000,
  })).stdout;
  const [tracked, untracked] = await Promise.all([
    git(['diff', '--no-ext-diff', '--no-textconv', '--no-renames', '--name-only', '-z', 'HEAD', '--']),
    git(['ls-files', '--others', '--exclude-standard', '-z']),
  ]);
  const files = [...new Set(`${tracked}${untracked}`.split('\0').filter(Boolean))]
    .filter((file) => !isManagedFile(file) && !ignoredNotebook(file, worktree, memoryPath));
  const readable = files.filter((file) => !isForbiddenRead(file));
  const diff = readable.length ? await git([
    '--literal-pathspecs', 'diff', '--no-ext-diff', '--no-textconv', '--no-renames',
    '--unified=0', 'HEAD', '--', ...readable,
  ]) : '';
  return { files, diff };
}

export async function checkExcellence({
  worktree, task, result, baseline, verifiedSnapshot, memoryPath, env, apiKeyEnv,
}) {
  const allowed = taskAndRepairFiles(taskFilesAllowed(task), result.repairFiles);
  const reasons = [];
  const options = { env, apiKeyEnv };
  if (result.error) reasons.push(redactEvidence(result.error.message, options));
  if (result.mode === 'stub') reasons.push('Deterministic stub: implementation and acceptance checks were not executed.');
  if (!result.tests && !taskSkipsTests(task)) reasons.push('Tests were not executed and TASK.md does not declare tests: none.');
  if (result.tests && result.tests.exit_code !== 0) reasons.push(`node --test failed (exit ${result.tests.exit_code}).`);
  if (!Number.isSafeInteger(result.turns) || result.turns < 0 ||
      (result.mode === 'llm' && result.turns === 0)) reasons.push('Coder turn count is missing or invalid.');
  if (typeof result.model !== 'string' || !/^[A-Za-z0-9._:/-]+$/.test(result.model) ||
      (result.mode === 'llm' && ['unknown', 'builtin-stub'].includes(result.model))) {
    reasons.push('Coder model ID is missing or invalid for AI-Run.');
  }
  if (typeof result.summary !== 'string' || !result.summary.trim()) reasons.push('Coder result summary is missing.');
  const current = await snapshotWorktree(worktree, { memoryPath });
  if (verifiedSnapshot && [...new Set([...verifiedSnapshot.keys(), ...current.keys()])]
    .some((file) => verifiedSnapshot.get(file) !== current.get(file))) {
    reasons.push('Worktree changed after final verification; rerun the coder checks before publication.');
  }
  const git = await gitChanges(worktree, memoryPath);
  const changed = baseline ? [...new Set([...baseline.keys(), ...current.keys()])]
    .filter((file) => baseline.get(file) !== current.get(file)) : [];
  const files = [...new Set([...changed, ...git.files])].sort();
  for (const file of files) {
    if (isForbiddenWrite(file) || !isAllowedFile(file, allowed)) {
      reasons.push(`Diff path is protected or outside TASK.md allowed paths: ${file}`);
      continue;
    }
    const target = path.join(worktree, file);
    await ensureLocalPath(target, worktree);
    let stat;
    try {
      stat = await fs.lstat(target);
    } catch (error) {
      if (error.code === 'ENOENT') continue;
      throw error;
    }
    if (!stat.isFile()) {
      reasons.push(`Diff path is not a regular file: ${file}`);
      continue;
    }
    const text = await fs.readFile(target, 'utf8');
    if (redactEvidence(text, options) !== text) reasons.push(`Secret material detected in changed file: ${file}`);
  }
  if (redactEvidence(git.diff, options) !== git.diff) reasons.push('Secret material detected in the Git diff.');
  return { pass: reasons.length === 0, reasons, files, model: result.model, turns: result.turns,
    snapshot: current };
}

export async function writeResult({ worktree, result, excellence, env, apiKeyEnv, run }) {
  const timedOut = result.timedOut === true;
  const blocked = result.blocked === true;
  const passed = excellence.pass && !timedOut && !blocked;
  const summary = timedOut ? 'Coder HTTP request timed out. No change was verified; this run did not complete.' : result.summary;
  const tests = result.tests ? `node --test exited ${result.tests.exit_code}`
    : result.testsSkipped ? 'Tests explicitly waived by TASK.md (tests: none).' : 'Tests were not run.';
  const body = '# Result\n\n' + (blocked ? 'Outcome: blocked (contracts infrastructure)\n\n'
    : timedOut ? 'Outcome: timed out (unverified)\n\n' : '') +
    `## Verification\n\nChecks: ${blocked ? 'BLOCKED' : passed ? 'PASS' : 'FAIL'}\n` +
    (passed ? '- Operational checks passed.\n'
      : excellence.reasons.map((reason, index) => `- ${index === 0 ? 'First failure: ' : ''}${reason}`).join('\n') + '\n') +
    `- ${tests}\n\n## Run\n\nModel: ${run?.metrics?.model ?? result.model}\nTool-loop turns: ${result.turns}\n` +
    `Research turns: ${result.research?.turns ?? 0}\n` +
    (result.testRepairs === undefined ? '' : `Test repairs: ${result.testRepairs} of 4\n` +
      `Additional failing-test scope: ${result.repairFiles?.join(', ') || '(none)'}\n`) +
    (result.implementationPath ? `Implementation path: ${result.implementationPath}\n` : '') +
    (result.stages ? `Stages: ${result.stages.join(' -> ')} -> result\n` : '') +
    (run ? `AI-Run: ${run.line}\n` : result.mode === 'stub'
      ? 'AI-Run: not emitted for a deterministic stub.\n' : 'AI-Run: unavailable; metadata validation failed.\n') +
    `\n## Files changed\n\n${excellence.files.map((file) => `- ${file}`).join('\n') || '(none)'}\n` +
    `\n## ${passed ? 'Summary' : 'Unverified summary'}\n\n${summary}\n`;
  const resultPath = path.join(worktree, 'RESULT.md');
  await ensureLocalPath(resultPath, worktree);
  await fs.writeFile(resultPath, redactEvidence(body, { env, apiKeyEnv }), {
    encoding: 'utf8', flag: 'wx', mode: 0o600,
  });
  return resultPath;
}
