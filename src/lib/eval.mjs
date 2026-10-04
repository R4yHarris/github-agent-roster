import { execFileSync } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { resolve } from 'node:path';
import {
  appendJsonl, failureDetail, IDENTIFIER, inferTaskClass, repositoryRoot, SHA,
  validateLocalEvaluation,
} from './learn.mjs';
import { githubRepository } from './issue.mjs';
import { loadAvailableMetrics } from './metrics.mjs';
import { splitArguments } from './arguments.mjs';

const usage = 'Use eval <sha-or-session> <accept|reject|rework> <1-5> <y|n> [--minutes N] [--comment "TEXT"].';

export function parseEvaluationArgs(input) {
  let args = input;
  if (typeof input === 'string') {
    args = splitArguments(input, usage);
  }
  if (!Array.isArray(args) || args.length < 4 || args.some((value) => typeof value !== 'string')) {
    throw new TypeError(usage);
  }
  const options = {};
  for (let index = 4; index < args.length; index += 2) {
    const flag = args[index];
    const value = args[index + 1];
    if (!['--minutes', '--comment'].includes(flag) || value === undefined ||
        Object.hasOwn(options, flag.slice(2))) throw new TypeError(usage);
    if (flag === '--minutes') {
      if (!/^(?:0|[1-9]\d*)$/.test(value) || !Number.isSafeInteger(Number(value))) {
        throw new TypeError('minutes must be a nonnegative integer');
      }
      options.minutes = Number(value);
    } else options.comment = value;
  }
  return { values: args.slice(0, 4), options };
}

async function commentEvaluation({ evaluation, record, cwd, run, env, log }) {
  const issue = /^roster-([1-9]\d*)-coder$/.exec(evaluation.session ?? '')?.[1];
  const branch = /^issue-[1-9]\d*$/.test(record?.task ?? '') ? record.task : issue && `issue-${issue}`;
  if (!evaluation.sha && !branch) return;
  const command = (program, args) => run(program, args, {
    cwd, env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
  });
  try {
    command('gh', ['--version']);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    log('GitHub CLI is unavailable; AI-Eval was saved locally only.');
    return;
  }
  let origin;
  try {
    origin = command('git', ['config', '--get', 'remote.origin.url']).trim();
  } catch (error) {
    if (error.status !== 1) throw error;
    log('No origin remote; AI-Eval was saved locally only.');
    return;
  }
  const repository = githubRepository(origin);
  const pulls = JSON.parse(evaluation.sha
    ? command('gh', ['api', `repos/${repository}/commits/${evaluation.sha}/pulls?per_page=2`])
    : command('gh', ['pr', 'list', '--repo', repository, '--head', branch, '--state', 'all',
      '--limit', '2', '--json', 'number']));
  if (!Array.isArray(pulls) || pulls.some((pull) => !Number.isSafeInteger(pull.number) || pull.number < 1)) {
    throw new Error('GitHub returned invalid matching PR data');
  }
  if (!pulls.length) {
    log('No matching PR; AI-Eval was saved locally only.');
    return;
  }
  if (pulls.length !== 1) throw new Error('Multiple PRs match this evaluation; post the human comment explicitly');
  if (command('gh', ['api', 'user', '--jq', '.type']).trim() !== 'User') {
    throw new Error('AI-Eval comments require a human GitHub identity, not a bot or App');
  }
  const body = `AI-Eval: 1|${evaluation.verdict}|${evaluation.difficulty}|${evaluation.again ? 'y' : 'n'}` +
    (evaluation.minutes === null ? '' : `\nMinutes: ${evaluation.minutes}`);
  command('gh', ['pr', 'comment', String(pulls[0].number), '--repo', repository, '--body', body]);
}

export async function recordEvaluation(target, verdict, difficulty, again, {
  cwd = process.cwd(),
  run = execFileSync,
  fileSystem = fs,
  env = process.env,
  minutes = null,
  comment = '',
  idempotent = false,
  now = () => new Date(),
  metricsLoader = loadAvailableMetrics,
  commenter = commentEvaluation,
  log = console.warn,
} = {}) {
  if (env.ROSTER_SEAT) throw new Error('AI-Eval is human-only; an agent seat cannot record an evaluation');
  if (typeof target !== 'string' || !IDENTIFIER.test(target) || target.startsWith('-')) {
    throw new TypeError('Use a Git SHA or an opaque 1-64 character session identifier.');
  }
  if (!/^[1-5]$/.test(String(difficulty))) {
    throw new TypeError('difficulty must be an integer from 1 to 5');
  }
  if (!['y', 'n'].includes(again)) throw new TypeError('again must be y or n');
  if (typeof comment !== 'string') throw new TypeError('comment must be text');
  const evaluation = {
    sha: SHA.test(target) ? target.toLowerCase() : null,
    session: SHA.test(target) ? null : target,
    model: null,
    task_class: null,
    verdict,
    difficulty: Number(difficulty),
    again: again === 'y',
    minutes, comment, at: now().toISOString(),
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
    evaluation.session = null;
    validateLocalEvaluation(evaluation, 'AI-Eval');
  }
  const records = metricsLoader({ cwd: root, run });
  const matches = records.filter((record) => evaluation.sha
    ? record.sha?.toLowerCase() === evaluation.sha
    : record.session === evaluation.session);
  const record = matches.find((match) => match.sha) ?? matches.at(-1);
  if (record) {
    evaluation.sha ??= record.sha?.toLowerCase() ?? null;
    evaluation.session ??= record.session ?? null;
    evaluation.model = ['unknown', 'builtin-stub'].includes(record.model) ? null : record.model ?? null;
    evaluation.task_class = record.task_class ?? inferTaskClass(record.task) ?? null;
    if (record.seat != null) evaluation.seat = record.seat;
  }
  validateLocalEvaluation(evaluation, 'AI-Eval');
  const directory = resolve(root, '.roster');
  try {
    await fileSystem.mkdir(directory, { recursive: true, mode: 0o700 });
  } catch (error) {
    throw new Error(`Could not create ${directory}: ${failureDetail(error)}`, { cause: error });
  }
  const file = resolve(directory, 'evals.jsonl');
  if (idempotent) {
    const learning = await fileSystem.readFile(file, 'utf8').catch((error) => {
      if (error.code === 'ENOENT') return '';
      throw error;
    });
    const existing = learning.split('\n').filter(Boolean).map((line) => JSON.parse(line))
      .find((item) => item.session === evaluation.session && item.session);
    if (existing) return { ...existing, path: file, duplicate: true };
  }
  await appendJsonl(file, evaluation, validateLocalEvaluation, fileSystem);
  try {
    await commenter({ evaluation, record, cwd: root, run, env, log });
  } catch (error) {
    throw new Error(`AI-Eval was recorded locally, but the PR comment failed: ${failureDetail(error)}`, { cause: error });
  }
  return { ...evaluation, path: file };
}
