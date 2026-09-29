import { execFileSync } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { resolve } from 'node:path';
import {
  appendJsonl, failureDetail, IDENTIFIER, repositoryRoot, SHA, validateLocalEvaluation,
} from './learn.mjs';

export async function recordEvaluation(target, verdict, difficulty, again, {
  cwd = process.cwd(),
  run = execFileSync,
  fileSystem = fs,
} = {}) {
  if (typeof target !== 'string' || !IDENTIFIER.test(target) || target.startsWith('-')) {
    throw new TypeError('Use a Git SHA or an opaque 1-64 character session identifier.');
  }
  if (!/^[1-5]$/.test(String(difficulty))) {
    throw new TypeError('difficulty must be an integer from 1 to 5');
  }
  if (!['y', 'n'].includes(again)) throw new TypeError('again must be y or n');
  const evaluation = {
    ...(SHA.test(target) ? { sha: target.toLowerCase() } : { session: target }),
    verdict,
    difficulty: Number(difficulty),
    again: again === 'y',
  };
  validateLocalEvaluation(evaluation, 'AI-Eval');
  const root = repositoryRoot(cwd, run);
  if (!SHA.test(target) && /^[0-9a-f]{4,63}$/i.test(target)) {
    try {
      evaluation.sha = run('git', ['rev-parse', '--verify', '--end-of-options', `${target}^{commit}`], {
        cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
      }).trim().toLowerCase();
    } catch (error) {
      throw new Error(`Could not resolve commit ${target}: ${failureDetail(error)}`, { cause: error });
    }
    delete evaluation.session;
    validateLocalEvaluation(evaluation, 'AI-Eval');
  }
  const directory = resolve(root, '.roster');
  try {
    await fileSystem.mkdir(directory, { recursive: true });
  } catch (error) {
    throw new Error(`Could not create ${directory}: ${failureDetail(error)}`, { cause: error });
  }
  await appendJsonl(resolve(directory, 'evals.jsonl'), evaluation, validateLocalEvaluation, fileSystem);
  return evaluation;
}
