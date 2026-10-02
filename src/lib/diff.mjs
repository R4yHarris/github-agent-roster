import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const execute = promisify(execFile);

export async function readDiffStatus({ cwd = process.cwd(), untracked = false, runCommand = execute } = {}) {
  if (typeof cwd !== 'string' || !cwd || typeof untracked !== 'boolean' || typeof runCommand !== 'function') {
    throw new TypeError('Diff status requires a working directory, boolean untracked option, and command runner');
  }
  const { stdout } = await runCommand('git', [
    'status', '--short', '--porcelain=v1', `--untracked-files=${untracked ? 'all' : 'no'}`,
  ], { cwd, encoding: 'utf8' });
  return stdout;
}
