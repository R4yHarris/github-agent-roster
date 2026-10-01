import { promises as fs } from 'node:fs';
import path from 'node:path';

export class ContractsSubmoduleError extends Error {
  code = 'ROSTER_CONTRACTS_UNINITIALIZED';

  constructor({ cause, tests } = {}) {
    super('Contracts submodule was not initialized', { cause });
    this.tests = tests;
  }
}

export async function assertContractsInitialized(worktree) {
  const modules = await fs.readFile(path.join(worktree, '.gitmodules'), 'utf8').catch((error) => {
    if (error.code === 'ENOENT') return null;
    throw error;
  });
  if (!modules || !/^\s*path\s*=\s*vendor[\\/]github-agent-contracts\s*$/m.test(modules)) return;
  const publisher = await fs.lstat(path.join(worktree, 'vendor', 'github-agent-contracts', 'scripts', 'agent-pr.mjs'))
    .catch((error) => {
      if (error.code === 'ENOENT') return null;
      throw error;
    });
  if (!publisher?.isFile() || publisher.isSymbolicLink()) throw new ContractsSubmoduleError();
}

export async function initializeWorktreeSubmodules(worktree, runCommand) {
  try {
    await runCommand('git', ['submodule', 'update', '--init', '--recursive'], worktree);
  } catch (error) {
    try {
      await assertContractsInitialized(worktree);
    } catch (dependencyError) {
      if (dependencyError instanceof ContractsSubmoduleError) throw new ContractsSubmoduleError({ cause: error });
      throw dependencyError;
    }
    throw error;
  }
  await assertContractsInitialized(worktree);
}

export function onlyMissingContractsScripts(tests) {
  if (tests?.exit_code === 0) return false;
  const output = `${tests?.stdout ?? ''}\n${tests?.stderr ?? ''}`.replace(/\\+/g, '/');
  const missing = [...output.matchAll(/Error(?: \[(?:ERR_MODULE_NOT_FOUND|MODULE_NOT_FOUND)\])?: Cannot find (?:module|package) ['"]([^'"]+)['"]/g)];
  if (!missing.length || missing.some(([, file]) =>
    !/(?:^|\/)vendor\/github-agent-contracts\/scripts\/[^/]+\.[cm]?js$/.test(file))) return false;
  if (/(?:AssertionError|TypeError|SyntaxError|ReferenceError|RangeError)(?: \[|\s*:)/.test(output)) return false;
  if (/code:\s*['"](?!ERR_TEST_FAILURE['"]|ERR_MODULE_NOT_FOUND['"]|MODULE_NOT_FOUND['"])[^'"]+['"]/.test(output)) return false;
  if (/error:\s*['"](?!test failed['"]|Cannot find (?:module|package)\b)[^'"]+['"]/.test(output)) return false;
  const otherErrors = output.replaceAll(/Error(?: \[(?:ERR_MODULE_NOT_FOUND|MODULE_NOT_FOUND)\])?: Cannot find (?:module|package) ['"][^'"]+['"]/g, '');
  if (/\bError(?: \[[^\]]+\])?:/.test(otherErrors)) return false;
  const failures = /(?:^|\n)\s*# fail (\d+)\s*(?:\n|$)/.exec(output);
  const failedCases = output.match(/(?:^|\n)\s*not ok \d+\b/g)?.length ?? 0;
  return Number(failures?.[1] ?? failedCases) <= missing.length;
}
