import { execFileSync } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { resolve } from 'node:path';
import { githubRepository } from './issue.mjs';
import {
  appendJsonl, failureDetail, IDENTIFIER, repositoryRoot, SHA, validateLocalEvaluation,
} from './learn.mjs';
import { loadMetrics } from './metrics.mjs';

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

function command(run, program, args, cwd, env) {
  const childEnv = { ...env, GH_PROMPT_DISABLED: '1' };
  delete childEnv.GITHUB_APP_ID;
  delete childEnv.GITHUB_APP_PRIVATE_KEY_PATH;
  return run(program, args, {
    cwd, env: childEnv, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
    timeout: 30_000, maxBuffer: 4 * 1024 * 1024,
  });
}

export async function postEvaluationComment(evaluation, {
  cwd = process.cwd(),
  env = process.env,
  run = execFileSync,
  metricsLoader = loadMetrics,
} = {}) {
  validateLocalEvaluation(evaluation, 'AI-Eval');
  const root = repositoryRoot(cwd, run);
  const remotes = command(run, 'git', ['remote'], root, env).trim().split(/\r?\n/);
  if (!remotes.includes('origin')) return { status: 'local', reason: 'no GitHub origin' };
  const repository = githubRepository(
    command(run, 'git', ['remote', 'get-url', 'origin'], root, env).trim());
  try {
    command(run, 'gh', ['--version'], root, env);
  } catch (error) {
    if (error?.code === 'ENOENT') return { status: 'local', reason: 'gh is not installed' };
    throw new Error('Could not check gh availability', { cause: error });
  }

  let shas = evaluation.sha ? [evaluation.sha] : null;
  if (shas === null) {
    const records = metricsLoader({ cwd: root });
    if (!Array.isArray(records)) throw new TypeError('Metrics must return run records');
    shas = [...new Set(records.filter((record) =>
      record?.session === evaluation.session && record.sha != null).map((record) =>
      String(record.sha).toLowerCase()))];
  }
  if (!shas.length) return { status: 'local', reason: 'no published commit for this session' };
  if (shas.some((sha) => !SHA.test(sha))) throw new Error('Run metrics contained an invalid commit SHA');

  const pullRequests = new Map();
  for (const sha of shas) {
    let associated;
    try {
      associated = JSON.parse(command(run, 'gh', [
        'api', `repos/${repository}/commits/${sha}/pulls?per_page=100`,
      ], root, env));
    } catch (error) {
      if (error instanceof SyntaxError) throw new Error('gh returned invalid PR association JSON', { cause: error });
      throw error;
    }
    if (!Array.isArray(associated) || associated.length === 100) {
      throw new Error('gh returned invalid or incomplete PR associations');
    }
    for (const pr of associated) {
      if (!Number.isSafeInteger(pr?.number) || pr.number <= 0 ||
          typeof pr.html_url !== 'string' || typeof pr.base?.repo?.full_name !== 'string') {
        throw new Error('gh returned an invalid associated PR');
      }
      if (pr.base.repo.full_name.toLowerCase() !== repository.toLowerCase()) continue;
      if (pr.html_url.toLowerCase() !==
          `https://github.com/${repository}/pull/${pr.number}`.toLowerCase()) {
        throw new Error('gh returned an associated PR outside the requested repository');
      }
      pullRequests.set(pr.number, pr);
    }
  }
  if (!pullRequests.size) return { status: 'local', reason: 'no associated PR' };
  if (pullRequests.size !== 1) throw new Error('Multiple PRs match this evaluation; no comment was posted');

  const [number] = pullRequests.keys();
  const body = `AI-Eval: 1|${evaluation.verdict}|${evaluation.difficulty}|${evaluation.again ? 'y' : 'n'}`;
  command(run, 'gh', ['pr', 'comment', String(number), '--repo', repository, '--body', body], root, env);
  return { status: 'commented', number };
}

export async function recordHumanEvaluation(target, verdict, difficulty, again, options = {}) {
  const evaluation = await recordEvaluation(target, verdict, difficulty, again, options);
  try {
    return { evaluation, comment: await postEvaluationComment(evaluation, options) };
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    throw new Error(`Recorded AI-Eval for ${evaluation.sha ?? evaluation.session} locally, ` +
      `but PR comment failed: ${error.message}`, { cause: error });
  }
}

export function formatEvaluationResult({ evaluation, comment }) {
  validateLocalEvaluation(evaluation, 'AI-Eval');
  const saved = `Recorded AI-Eval for ${evaluation.sha ?? evaluation.session}.\n`;
  if (comment?.status === 'commented' && Number.isSafeInteger(comment.number) && comment.number > 0) {
    return saved + `Commented AI-Eval: 1|${evaluation.verdict}|${evaluation.difficulty}|` +
      `${evaluation.again ? 'y' : 'n'} on PR #${comment.number}.\n`;
  }
  if (comment?.status === 'local' && typeof comment.reason === 'string' && comment.reason) {
    return saved + `No PR comment: ${comment.reason}.\n`;
  }
  throw new TypeError('Expected a GitHub comment result');
}
