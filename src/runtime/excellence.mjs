import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { ensureLocalPath } from '../lib/paths.mjs';
import { redactEvidence } from '../lib/redaction.mjs';
import { taskFilesAllowed } from '../planner/stub.mjs';
import { splitTaskFrontmatter } from './skills.mjs';
import { addedLinesByFile, addsOnlyImports, addsTestEvidence, analyzeTestSubstance, isTestFile } from './test-substance.mjs';
import { readTaskMetadata } from './estimate.mjs';
import { isAllowedFile, isForbiddenRead, isForbiddenWrite, isManagedFile, isRunLog, isDebugLog, isShellHistory, isCheckpoint, isRepoMap, taskAndRepairFiles } from './tools.mjs';

const execute = promisify(execFile);
export { redactEvidence } from '../lib/redaction.mjs';

export function taskSkipsTests(task) {
  const { frontmatter } = splitTaskFrontmatter(task);
  const declarations = frontmatter.split('\n').filter((line) => /^tests:/.test(line));
  if (declarations.length > 1) throw new Error('TASK.md must not repeat tests');
  if (!declarations.length) return false;
  const value = /^tests: (none|required)[ \t]*$/.exec(declarations[0]);
  if (!value) throw new Error('TASK.md tests must be none or required');
  return value[1] === 'none';
}

function ignoredNotebook(file, worktree, memoryPath) {
  return isRunLog(file) || isDebugLog(file) || isShellHistory(file) || isCheckpoint(file) || isRepoMap(file) || file === '.roster/memory' || file.startsWith('.roster/memory/') ||
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
    if (error.code === 'ENOENT') return { files: [], diff: '', hasGit: false };
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
  return { files, diff, hasGit: true };
}

function isVendorMetadata(file) {
  const normalized = file.replaceAll('\\', '/');
  return normalized === 'vendor' || normalized.startsWith('vendor/') && normalized.split('/').includes('.git');
}

function taskClass(task) {
  try {
    return readTaskMetadata(task).task_class;
  } catch {
    return null;
  }
}

export async function checkExcellence({
  worktree, task, result, baseline, verifiedSnapshot, memoryPath, env, apiKeyEnv,
}) {
  const allowed = taskAndRepairFiles(taskFilesAllowed(task), result.repairFiles, result.scopeFiles);
  const reasons = [];
  const options = { env, apiKeyEnv };
  if (result.error) reasons.push(redactEvidence(result.error.message, options));
  if (result.mode === 'stub') reasons.push('Deterministic stub: implementation and acceptance checks were not executed.');
  if (!result.tests && !result.testsSkipped && !taskSkipsTests(task)) reasons.push('Tests were not executed and TASK.md does not declare tests: none.');
  if (result.tests && !result.tests.skipped && result.tests.exit_code !== 0 && !result.baselineFailures?.length) {
    reasons.push(`node --test failed (exit ${result.tests.exit_code}).`);
  }
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
  const files = [...new Set([...changed, ...git.files].filter((file) => {
    const normalized = file.replaceAll('\\', '/');
    return !isVendorMetadata(file) && normalized.split('/')[0].toLowerCase() !== 'vendor';
  }))].sort();
  const scoped = taskFilesAllowed(task);
  if (result.mode === 'llm' && scoped.length === 1 && !scoped[0].endsWith('.md') &&
      !(git.hasGit ? git.files : files).some((file) => isAllowedFile(file, scoped))) {
    reasons.push(`Bounded task must produce an application diff in ${scoped[0]}; passing existing tests or rewriting identical content does not implement the Ask.`);
  }
  let added;
  let testEvidence = false;
  let testAdditions = '';
  for (const file of files) {
    if (isForbiddenWrite(file)) {
      reasons.push(`Diff path is protected or outside TASK.md allowed paths: ${file}`);
      continue;
    }
    if (!isAllowedFile(file, allowed)) {
      reasons.push(`Diff path is outside TASK.md allowed paths: ${file}`);
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
    if (result.mode === 'llm' && git.hasGit && isTestFile(file)) {
      added ??= addedLinesByFile(git.diff);
      const fileAdded = added.get(file) ?? text;
      testEvidence ||= addsTestEvidence(fileAdded);
      testAdditions += `${fileAdded}\n`;
      reasons.push(...analyzeTestSubstance({ file, text, added: fileAdded }));
    }
  }
  const nonTestChanged = files.some((file) => !isTestFile(file) && isAllowedFile(file, allowed) && !isForbiddenWrite(file));
  if (result.mode === 'llm' && git.hasGit && !testEvidence && testAdditions.trim() && !nonTestChanged &&
      (taskClass(task) === 'test' || addsOnlyImports(testAdditions))) {
    reasons.push('Test substance: the diff changes only test files but adds no new test block or assertion; ' +
      'imports, comments, or fixtures alone do not implement the Ask.');
  }
  if (redactEvidence(git.diff, options) !== git.diff) reasons.push('Secret material detected in the Git diff.');
  return { pass: reasons.length === 0, reasons, files, model: result.model, turns: result.turns,
    snapshot: current };
}

const testNamePattern = /\b(?:test|it)\(\s*(['"`])((?:\\.|(?!\1)[^\\])*)\1/g;

// Names of the tests a changed test file declares, so its results can be quoted from the full-suite output.
export function declaredTestNames(text) {
  return [...String(text).matchAll(testNamePattern)]
    .filter(([, quote, name]) => !(quote === '`' && name.includes('${')))
    .map(([, , name]) => name.replace(/\\(.)/g, '$1'));
}

// A large suite's head is unrelated tests: quote the totals and every result line from the changed test files.
export function testEvidence(tests, changedTests = []) {
  const output = [tests.stdout, tests.stderr].filter((part) => typeof part === 'string' && part.trim()).join('\n').trim();
  const lines = output.split(/\r?\n/);
  const totals = lines.filter((line) => /^ℹ (tests|suites|pass|fail|cancelled|skipped|todo) \d+$/.test(line.trim()));
  const changed = changedTests.map(({ file, names }) => {
    const results = lines.map((line) => line.trim()).filter((line) => /^[✔✖﹣] /.test(line) &&
      names.some((name) => line.slice(2).startsWith(`${name} (`) || line.slice(2) === name));
    return `${file}: ${results.length} of ${names.length} declared tests reported\n` +
      [...new Set(results)].slice(0, 40).map((line) => `  ${line}`).join('\n');
  }).join('\n').slice(0, 4000);
  const excerpt = (output || '(no output)').slice(0, 1200);
  return `node --test exited ${tests.exit_code}\nCommand: node --test\nExit code: ${tests.exit_code}\n` +
    (totals.length ? `Totals:\n${totals.map((line) => `  ${line.trim()}`).join('\n')}\n` : '') +
    (changed ? `Changed test files:\n${changed}\n` : '') +
    `Output:\n${excerpt}`;
}

async function changedTestNames(worktree, files) {
  const changed = [];
  for (const file of files.filter(isTestFile)) {
    try {
      const names = declaredTestNames(await fs.readFile(path.join(worktree, file), 'utf8'));
      if (names.length) changed.push({ file, names });
    } catch {}
  }
  return changed;
}

export async function writeResult({ worktree, result, excellence, env, apiKeyEnv, run }) {
  const timedOut = result.timedOut === true;
  const blocked = result.blocked === true;
  const passed = excellence.pass && !timedOut && !blocked;
  const summary = timedOut ? 'Coder HTTP request timed out. No change was verified; this run did not complete.' : result.summary;
  const tests = result.tests?.skipped ? `Tests skipped: ${result.tests.stdout}`
    : result.tests ? testEvidence(result.tests, await changedTestNames(worktree, excellence.files ?? []))
    : result.testsSkipped ? 'Tests skipped: docs-only change is checked by reading the file.' : 'Tests were not run.';
  const body = '# Result\n\n' + (blocked ? 'Outcome: blocked (contracts infrastructure)\n\n'
    : timedOut ? 'Outcome: timed out (unverified)\n\n' : '') +
    `## Verification\n\nChecks: ${blocked ? 'BLOCKED' : passed ? 'PASS' : 'FAIL'}\n` +
    (passed ? '- Operational checks passed.\n'
      : excellence.reasons.map((reason, index) => `- ${index === 0 ? 'First failure: ' : ''}${reason}`).join('\n') + '\n') +
    `- ${tests}\n\n## Run\n\nModel: ${run?.metrics?.model ?? result.model}\nTool-loop turns: ${result.turns}\n` +
    `Research turns: ${result.research?.turns ?? 0}\n` +
    (result.testRepairs === undefined ? '' : `Test repairs: ${result.testRepairs} of ${result.testRepairBudget ?? 4}\n` +
      `Additional failing-test scope: ${result.repairFiles?.join(', ') || '(none)'}\n`) +
    (result.scopeFiles?.length ? `Files outside planned scope: ${result.scopeFiles.join(', ')}\n` : '') +
    (result.noProgressRepairUsed ? 'No-progress corrections: 1 of 1\n' : '') +
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
