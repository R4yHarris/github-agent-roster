import { execFileSync } from 'node:child_process';
import { promises as fs, readFileSync, readdirSync, statSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { ensureLocalPath, resolveContractsPath } from './paths.mjs';
import { materializeRun } from '../metrics/run.mjs';

export const EFFORTS = ['l', 'm', 'h', 'x'];
export const TASK_CLASSES = ['feat', 'fix', 'docs', 'test'];
export const VERDICTS = ['accept', 'reject', 'rework'];
export const SHA = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i;
export const IDENTIFIER = /^(?!-$)[A-Za-z0-9._-]{1,64}$/;
const RUN_FIELDS = [
  'sha', 'session', 'task', 'task_class', 'provider', 'model', 'effort',
  'prompt_tokens', 'completion_tokens', 'context_used', 'context_max', 'context_out', 'excellence', 'defects',
];

export function isObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

export function failureDetail(error) {
  const stderr = isObject(error) && error.stderr != null ? String(error.stderr).trim() : '';
  const message = error instanceof Error ? error.message : String(error);
  return stderr ? `${stderr} (${message})` : message;
}

export function validateSha(record, source) {
  if (typeof record.sha !== 'string' || !SHA.test(record.sha)) {
    throw new Error(`${source}: sha must be a full 40- or 64-character hexadecimal Git SHA`);
  }
}

export function parseJsonl(text, source, validate, uniqueSha = false) {
  if (typeof text !== 'string') throw new TypeError(`${source}: expected UTF-8 JSONL text`);
  if (text === '') return [];
  const lines = text.split(/\r?\n/);
  if (lines.at(-1) === '') lines.pop();
  const seen = new Set();
  return lines.map((line, index) => {
    const location = `${source}:${index + 1}`;
    if (!line.trim()) throw new Error(`${location}: blank JSONL line`);
    let record;
    try {
      record = JSON.parse(line);
    } catch (error) {
      throw new Error(`${location}: invalid JSON`, { cause: error });
    }
    validate(record, location);
    if (uniqueSha) {
      const sha = record.sha.toLowerCase();
      if (seen.has(sha)) throw new Error(`${location}: duplicate sha ${record.sha}`);
      seen.add(sha);
    }
    return record;
  });
}

function validateLocalRun(record, source) {
  if (!isObject(record)) throw new Error(`${source}: expected a JSON object`);
  if (record.sha == null && record.session == null) {
    throw new Error(`${source}: a run needs a sha or session`);
  }
  if (record.sha != null) validateSha(record, source);
  for (const field of ['session', 'task']) {
    if (record[field] != null &&
        (typeof record[field] !== 'string' || !IDENTIFIER.test(record[field]))) {
      throw new Error(`${source}: ${field} must be an opaque 1-64 character identifier`);
    }
  }
  if (record.model != null &&
      (typeof record.model !== 'string' || !record.model || /\s/.test(record.model))) {
    throw new Error(`${source}: model must be a nonempty name without whitespace`);
  }
  if (record.provider != null &&
      !['vllm', 'github-copilot', 'anthropic', 'openai', 'local', 'other'].includes(record.provider)) {
    throw new Error(`${source}: provider must name a supported model backend`);
  }
  if (record.effort != null && !EFFORTS.includes(record.effort)) {
    throw new Error(`${source}: effort must be l, m, h, x, or null`);
  }
  if (record.task_class != null && !TASK_CLASSES.includes(record.task_class)) {
    throw new Error(`${source}: task_class must be feat, fix, docs, or test`);
  }
  for (const field of ['context_used', 'context_max', 'context_out']) {
    const value = record[field];
    if (value != null && !(Number.isSafeInteger(value) && value >= 0) &&
        !(typeof value === 'string' && /^\d+$/.test(value))) {
      throw new Error(`${source}: ${field} must be a nonnegative integer or decimal string`);
    }
    for (const field of ['prompt_tokens', 'completion_tokens']) {
      if (record[field] != null && (!Number.isSafeInteger(record[field]) || record[field] < 0)) {
        throw new Error(`${source}: ${field} must be a reported nonnegative safe integer`);
      }
      for (const [alias, field] of [['context_used', 'prompt_tokens'], ['context_out', 'completion_tokens']]) {
        if (record[alias] != null && record[field] != null && String(record[alias]) !== String(record[field])) {
          throw new Error(`${source}: token aliases must match reported usage`);
        }
      }
    }
  }
  if (record.excellence != null && !['pass', 'fail'].includes(record.excellence) &&
      (!isObject(record.excellence) || typeof record.excellence.pass !== 'boolean')) {
    throw new Error(`${source}: excellence must be pass, fail, or a report with boolean pass`);
  }
  if (record.defects != null && (!Array.isArray(record.defects) ||
      record.defects.some((reason) => typeof reason !== 'string' || !reason.trim() ||
        /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(reason)))) {
    throw new Error(`${source}: defects must be an array of nonempty failure reasons without control characters`);
  }
}

export function validateLocalEvaluation(record, source) {
  if (!isObject(record)) throw new Error(`${source}: expected a JSON object`);
  if (record.sha == null && record.session == null) {
    throw new Error(`${source}: an evaluation needs a sha or session`);
  }
  if (record.sha != null) validateSha(record, source);
  if (record.session != null &&
      (typeof record.session !== 'string' || !IDENTIFIER.test(record.session))) {
    throw new Error(`${source}: session must be an opaque 1-64 character identifier`);
  }
  if (!VERDICTS.includes(record.verdict)) {
    throw new Error(`${source}: verdict must be accept, reject, or rework`);
  }
  if (!Number.isInteger(record.difficulty) || record.difficulty < 1 || record.difficulty > 5) {
    throw new Error(`${source}: difficulty must be an integer from 1 to 5`);
  }
  if (typeof record.again !== 'boolean') throw new Error(`${source}: again must be a boolean`);
  validateLocalRun(record, source);
  if (record.minutes != null && (!Number.isSafeInteger(record.minutes) || record.minutes < 0)) {
    throw new Error(`${source}: minutes must be a nonnegative integer`);
  }
  if (record.comment != null && (typeof record.comment !== 'string' ||
      Buffer.byteLength(record.comment, 'utf8') > 4096 || /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(record.comment))) {
    throw new Error(`${source}: comment must be text of at most 4 KiB without control characters`);
  }
  if (record.at != null && (typeof record.at !== 'string' ||
      !Number.isFinite(Date.parse(record.at)) || new Date(record.at).toISOString() !== record.at)) {
    throw new Error(`${source}: at must be an ISO timestamp`);
  }
}

export function inferTaskClass(task) {
  return typeof task === 'string'
    ? /^(feat|fix|docs|test)(?:$|[-_.:]|\([^)]+\)!?:|!:)/.exec(task)?.[1]
    : undefined;
}

export function repositoryRoot(cwd = process.cwd(), run = execFileSync) {
  try {
    const root = run('git', ['rev-parse', '--show-toplevel'], {
      cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
    }).trim();
    if (!root) throw new Error('Git returned no repository root');
    return resolve(root);
  } catch (error) {
    throw new Error(`Could not find Git repository root: ${failureDetail(error)}`, { cause: error });
  }
}

export async function appendJsonl(file, record, validate, fileSystem = fs) {
  let previous = '';
  try {
    previous = await fileSystem.readFile(file, 'utf8');
  } catch (error) {
    if (error.code !== 'ENOENT') {
      throw new Error(`Could not read ${file}: ${failureDetail(error)}`, { cause: error });
    }
  }
  parseJsonl(previous, file, validate);
  const separator = previous && !previous.endsWith('\n') ? '\n' : '';
  try {
    await fileSystem.appendFile(file, `${separator}${JSON.stringify(record)}\n`, {
      encoding: 'utf8', mode: 0o600,
    });
  } catch (error) {
    throw new Error(`Could not append ${file}: ${failureDetail(error)}`, { cause: error });
  }
}

function partialRunMetadata(env) {
  const metadata = {};
  const effort = env.AI_EFFORT;
  if (effort !== undefined && effort !== '' && effort !== '-') {
    const aliases = { low: 'l', medium: 'm', high: 'h', max: 'x' };
    if (typeof effort !== 'string' || (!EFFORTS.includes(effort) && !Object.hasOwn(aliases, effort))) {
      throw new TypeError('AI_EFFORT must be l, m, h, x, low, medium, high, or max');
    }
    metadata.effort = Object.hasOwn(aliases, effort) ? aliases[effort] : effort;
  }
  for (const [name, field] of [
    ['AI_CONTEXT_USED', 'context_used'], ['AI_CONTEXT_MAX', 'context_max'], ['AI_CONTEXT_OUT', 'context_out'],
  ]) {
    const value = env[name];
    if (value === undefined || value === '' || value === '-') continue;
    if (typeof value !== 'string' || !/^\d+$/.test(value)) {
      throw new TypeError(`${name} must be a nonnegative decimal integer`);
    }
    const count = Number(value);
    metadata[field] = Number.isSafeInteger(count) ? count : value;
  }
  return metadata;
}

export async function recordRun(record, {
  cwd = process.cwd(),
  env = process.env,
  fileSystem = fs,
  createDirectory = false,
  run,
} = {}) {
  if (typeof createDirectory !== 'boolean') throw new TypeError('createDirectory must be a boolean');
  const directory = resolve(cwd, '.roster', 'runs');
  if (createDirectory) {
    await ensureLocalPath(directory, cwd);
    try {
      await fileSystem.mkdir(directory, { recursive: true, mode: 0o700 });
    } catch (error) {
      throw new Error(`Could not create ${directory}: ${failureDetail(error)}`, { cause: error });
    }
    await ensureLocalPath(directory, cwd);
  }
  let status;
  try {
    status = await fileSystem.stat(directory);
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw new Error(`Could not inspect ${directory}: ${failureDetail(error)}`, { cause: error });
  }
  if (!status.isDirectory()) throw new Error(`${directory} must be a directory`);

  const parser = resolve(resolveContractsPath({ env, cwd }), 'scripts', 'parse-agent-run.mjs');
  const { packAgentRun, parseAgentRun } = await import(pathToFileURL(parser).href);
  const completed = run?.metrics ? materializeRun(run.metrics, run.version) : null;
  for (const field of ['session', 'task']) {
    if (completed?.metrics[field] !== undefined && record[field] !== undefined &&
        completed.metrics[field] !== record[field]) {
      throw new TypeError('A completed run session/task cannot be changed while recording');
    }
  }
  const model = completed?.metrics.model ?? record.model ?? env.AI_MODEL;
  // Model-free local journals retain partial evidence without packing a fabricated AI-Run.
  const metadata = completed ? parseAgentRun(packAgentRun(completed.env, model)) : model ? parseAgentRun(packAgentRun({
    ...env, AI_PROVIDER: env.AI_PROVIDER === 'vllm' ? 'local' : env.AI_PROVIDER,
    AI_MODEL: model, AI_SESSION: record.session ?? '', AI_TASK: record.task ?? '',
  }, model)) : partialRunMetadata(env);
  const measuredFields = ['provider', 'model', 'effort', 'prompt_tokens', 'completion_tokens',
    'context_used', 'context_max', 'context_out'];
  const additions = completed ? Object.fromEntries(Object.entries(record)
    .filter(([field]) => !measuredFields.includes(field))) : record;
  const supplied = {
    ...metadata, ...(!completed && model && env.AI_PROVIDER === 'vllm' ? { provider: 'vllm' } : {}), ...additions,
    ...completed?.metrics,
  };
  const stored = Object.fromEntries(RUN_FIELDS
    .filter((field) => supplied[field] != null)
    .map((field) => [field, supplied[field]]));
  const file = resolve(directory, 'runs.jsonl');
  validateLocalRun(stored, file);
  await appendJsonl(file, stored, validateLocalRun, fileSystem);
  return stored;
}

export function loadLearning({ cwd = process.cwd(), readFile = readFileSync } = {}) {
  const directory = resolve(cwd, '.roster', 'runs');
  let entries;
  try {
    entries = readdirSync(directory, { withFileTypes: true });
  } catch (error) {
    if (error.code !== 'ENOENT') {
      throw new Error(`Could not read ${directory}: ${failureDetail(error)}`, { cause: error });
    }
    entries = [];
  }
  function read(file, validate) {
    let text;
    try {
      text = readFile(file, 'utf8');
    } catch (error) {
      throw new Error(`Could not read ${file}: ${failureDetail(error)}`, { cause: error });
    }
    return parseJsonl(text, file, validate);
  }
  const runs = entries.filter((entry) => entry.name.endsWith('.jsonl'))
    .sort((left, right) => left.name.localeCompare(right.name))
    .flatMap((entry) => {
      const file = resolve(directory, entry.name);
      if (!entry.isFile()) throw new Error(`${file} must be a JSONL file`);
      return read(file, validateLocalRun);
    });
  const evalsFile = resolve(cwd, '.roster', 'evals.jsonl');
  const status = statSync(evalsFile, { throwIfNoEntry: false });
  if (status && !status.isFile()) throw new Error(`${evalsFile} must be a JSONL file`);
  return { runs, evaluations: status ? read(evalsFile, validateLocalEvaluation) : [] };
}

function knownFields(record) {
  return Object.fromEntries(Object.entries(record).filter(([key, value]) =>
    value != null && !(key === 'model' && value === 'unknown')));
}

function reportFailed(record) {
  return record.excellence === 'fail' || record.excellence?.pass === false;
}

export function excellenceFailed(record) {
  return reportFailed(record) || Boolean(record.defects?.length);
}

function mergeDefects(...records) {
  return [...new Set(records.flatMap((record) => record.defects ?? []))];
}

export function matchesEvaluation(record, evaluation) {
  if (record.sha && evaluation.sha) return record.sha.toLowerCase() === evaluation.sha.toLowerCase();
  return Boolean(record.session && record.session === evaluation.session);
}

export function joinLearning(exported, local, evaluations, includeUnpublished = true) {
  const records = exported.map((record) => ({ record: { ...record }, git: record }));
  for (const run of local) {
    const matches = records.filter(({ record }) => {
      if (run.sha && record.sha) return run.sha.toLowerCase() === record.sha.toLowerCase();
      return run.session && run.session === record.session &&
        (!run.task || !record.task || run.task === record.task);
    });
    if (matches.length) {
      for (const entry of matches) {
        const failure = reportFailed(run) ? run.excellence
          : reportFailed(entry.record) ? entry.record.excellence : undefined;
        const defects = mergeDefects(entry.record, run);
        const vllm = run.provider === 'vllm' || entry.record.provider === 'vllm';
        entry.record = { ...entry.record, ...knownFields(run), ...knownFields(entry.git ?? {}) };
        if (vllm && (!entry.git?.provider || entry.git.provider === 'local')) {
          entry.record.provider = 'vllm';
        }
        if (failure !== undefined) entry.record.excellence = failure;
        if (defects.length) entry.record.defects = defects;
      }
    } else if (includeUnpublished) {
      records.push({ record: { ...run } });
    }
  }
  const bySha = new Map();
  const bySession = new Map();
  for (const evaluation of evaluations) {
    if (evaluation.sha) bySha.set(evaluation.sha.toLowerCase(), evaluation);
    if (evaluation.session) bySession.set(evaluation.session, evaluation);
    const metadata = knownFields(Object.fromEntries(RUN_FIELDS
      .filter((field) => Object.hasOwn(evaluation, field))
      .map((field) => [field, evaluation[field]])));
    const matches = records.filter(({ record }) => matchesEvaluation(record, evaluation));
    for (const entry of matches) {
      const failure = [metadata, entry.record].find(reportFailed)?.excellence;
      const defects = mergeDefects(entry.record, metadata);
      entry.record = entry.evaluationOnly ? { ...entry.record, ...metadata }
        : { ...entry.record, ...metadata, ...knownFields(entry.record) };
      if (failure !== undefined) entry.record.excellence = failure;
      if (defects.length) entry.record.defects = defects;
    }
    if (!matches.length && includeUnpublished && metadata.model && metadata.task_class) {
      records.push({ record: metadata, evaluationOnly: true });
    }
  }
  return records.map(({ record }) => {
    const sessionEvaluation = bySession.get(record.session);
    return {
      ...record,
      evaluation: bySha.get(record.sha?.toLowerCase()) ??
        (sessionEvaluation && matchesEvaluation(record, sessionEvaluation) ? sessionEvaluation : null),
    };
  });
}

export function median(values) {
  if (!Array.isArray(values) || values.some((value) => !Number.isFinite(value))) {
    throw new TypeError('Median requires finite numeric samples');
  }
  if (!values.length) return null;
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle]
    : sorted[middle - 1] + (sorted[middle] - sorted[middle - 1]) / 2;
}

export function summarizeLearning(records) {
  if (!Array.isArray(records)) throw new TypeError('Expected an array of joined metrics records.');
  const groups = new Map();
  for (const record of records) {
    const model = record.model ?? null;
    const taskClass = record.task_class ?? inferTaskClass(record.task) ?? null;
    const effort = record.effort ?? null;
    const key = JSON.stringify([model, taskClass, effort]);
    if (!groups.has(key)) {
      groups.set(key, { model, task_class: taskClass, effort, runs: 0, evaluated: 0, samples: new Map() });
    }
    const group = groups.get(key);
    group.runs += 1;
    const evaluation = record.evaluation;
    if (evaluation != null) group.evaluated += 1;
    const failed = excellenceFailed(record);
    const verdict = failed ? 'reject' : evaluation?.verdict;
    if (!VERDICTS.includes(verdict)) continue;
    const target = evaluation ?? record;
    const id = target.sha ? `sha:${target.sha.toLowerCase()}` : target.session && `session:${target.session}`;
    if (!id) throw new TypeError('An evaluated result needs a SHA or session');
    const prior = group.samples.get(id);
    group.samples.set(id, {
      verdict: failed || prior?.failed ? 'reject' : verdict,
      failed: failed || prior?.failed || false,
      minutes: Number.isSafeInteger(evaluation?.minutes) && evaluation.minutes >= 0 ? evaluation.minutes : null,
      difficulty: Number.isInteger(evaluation?.difficulty) &&
        evaluation.difficulty >= 1 && evaluation.difficulty <= 5 ? evaluation.difficulty : null,
    });
  }
  return [...groups.values()].map(({ samples, ...group }) => {
    const values = [...samples.values()];
    const accepted = values.filter(({ verdict }) => verdict === 'accept');
    const acceptedMinutes = median(accepted.map(({ minutes }) => minutes).filter((value) => value !== null));
    return {
      ...group, n: values.length, accepted: accepted.length,
      acceptRate: values.length ? accepted.length / values.length : null,
      medianMinutes: median(values.map(({ minutes }) => minutes).filter((value) => value !== null)),
      medianDifficulty: median(values.map(({ difficulty }) => difficulty).filter((value) => value !== null)),
      estimate_min: acceptedMinutes === null ? null : Math.round(acceptedMinutes),
    };
  }).sort((left, right) => (left.model ?? '').localeCompare(right.model ?? '') ||
    (left.task_class ?? '').localeCompare(right.task_class ?? '') ||
    [...EFFORTS, null].indexOf(left.effort) - [...EFFORTS, null].indexOf(right.effort));
}

export function recommend(records, taskClass, difficulty = null) {
  if (!TASK_CLASSES.includes(taskClass)) {
    throw new TypeError('task class must be feat, fix, docs, or test');
  }
  if (difficulty !== null && (!Number.isInteger(difficulty) || difficulty < 1 || difficulty > 5)) {
    throw new TypeError('difficulty must be an integer from 1 to 5');
  }
  const eligible = summarizeLearning(records).filter((group) =>
    group.task_class === taskClass && group.model && !['unknown', 'builtin-stub'].includes(group.model) &&
    group.n >= 3 && (difficulty === null || (group.medianDifficulty !== null && group.medianDifficulty >= difficulty)))
    .map(({ model, effort, n, accepted, acceptRate, medianMinutes, medianDifficulty, estimate_min }) => ({
      model, effort, n, accepted, acceptRate, medianMinutes, medianDifficulty, estimate_min,
    }));
  eligible.sort((left, right) => right.acceptRate - left.acceptRate || right.n - left.n ||
    left.model.localeCompare(right.model) ||
    [...EFFORTS, null].indexOf(left.effort) - [...EFFORTS, null].indexOf(right.effort));
  return eligible[0] ?? null;
}

export function parseRecommendationArgs(args) {
  const usage = 'Use recommend --task-class feat|fix|docs|test [--difficulty 1-5].';
  if (!Array.isArray(args) || args.some((value) => typeof value !== 'string')) throw new TypeError(usage);
  const values = new Map();
  for (let index = 0; index < args.length; index += 2) {
    const flag = args[index];
    if (!['--task-class', '--difficulty'].includes(flag) || args[index + 1] === undefined || values.has(flag)) {
      throw new TypeError(usage);
    }
    values.set(flag, args[index + 1]);
  }
  const taskClass = values.get('--task-class');
  const difficulty = values.has('--difficulty') ? values.get('--difficulty') : null;
  if (!TASK_CLASSES.includes(taskClass) || (difficulty !== null && !/^[1-5]$/.test(difficulty))) {
    throw new TypeError(usage);
  }
  return { taskClass, difficulty: difficulty === null ? null : Number(difficulty) };
}

export function formatRecommendation(recommendation, taskClass, config, env = process.env) {
  if (!recommendation) {
    return `insufficient data; config default: ${config?.llm?.model || env.ROSTER_MODEL || '(unset)'} effort=${config?.llm?.effort ?? '-'}\n`;
  }
  const { model, effort, acceptRate, n, medianMinutes, medianDifficulty } = recommendation;
  return `${taskClass}: ${model} effort=${effort ?? '-'} accept-rate=${(acceptRate * 100).toFixed(1)}% n=${n}` +
    ` median-min=${medianMinutes ?? '-'} median-difficulty=${medianDifficulty ?? '-'}\n`;
}
