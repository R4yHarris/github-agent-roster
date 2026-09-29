import { execFileSync } from 'node:child_process';
import { promises as fs, readFileSync, readdirSync, statSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { ensureLocalPath, resolveContractsPath } from './paths.mjs';

export const EFFORTS = ['l', 'm', 'h', 'x'];
export const TASK_CLASSES = ['feat', 'fix', 'docs', 'test'];
export const VERDICTS = ['accept', 'reject', 'rework'];
export const SHA = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/i;
export const IDENTIFIER = /^(?!-$)[A-Za-z0-9._-]{1,64}$/;
const RUN_FIELDS = [
  'sha', 'session', 'task', 'task_class', 'model', 'effort',
  'context_used', 'context_max', 'context_out',
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

export async function recordRun(record, {
  cwd = process.cwd(),
  env = process.env,
  fileSystem = fs,
  createDirectory = false,
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
  const metadata = parseAgentRun(packAgentRun({
    ...env, AI_SESSION: record.session ?? '', AI_TASK: record.task ?? '',
  }, env.AI_MODEL || 'unknown'));
  const supplied = { ...metadata, model: env.AI_MODEL ? metadata.model : undefined, ...record };
  const run = Object.fromEntries(RUN_FIELDS
    .filter((field) => supplied[field] != null)
    .map((field) => [field, supplied[field]]));
  const file = resolve(directory, 'runs.jsonl');
  validateLocalRun(run, file);
  await appendJsonl(file, run, validateLocalRun, fileSystem);
  return run;
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
        entry.record = { ...entry.record, ...knownFields(run), ...knownFields(entry.git ?? {}) };
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
  }
  return records.map(({ record }) => ({
    ...record,
    evaluation: bySha.get(record.sha?.toLowerCase()) ?? bySession.get(record.session) ?? null,
  }));
}

export function recommend(records, taskClass) {
  if (!TASK_CLASSES.includes(taskClass)) {
    throw new TypeError('task class must be feat, fix, docs, or test');
  }
  const groups = new Map();
  for (const record of records) {
    if ((record.task_class ?? inferTaskClass(record.task)) !== taskClass ||
        !record.model || ['unknown', 'builtin-stub'].includes(record.model) ||
        !VERDICTS.includes(record.evaluation?.verdict)) {
      continue;
    }
    const effort = record.effort ?? null;
    const key = JSON.stringify([record.model, effort]);
    if (!groups.has(key)) {
      groups.set(key, { model: record.model, effort, samples: new Map() });
    }
    const evaluation = record.evaluation;
    const id = evaluation.sha ? `sha:${evaluation.sha.toLowerCase()}` : `session:${evaluation.session}`;
    groups.get(key).samples.set(id, evaluation.verdict);
  }
  const eligible = [...groups.values()].filter(({ samples }) => samples.size >= 3)
    .map(({ model, effort, samples }) => {
      const accepted = [...samples.values()].filter((verdict) => verdict === 'accept').length;
      return { model, effort, n: samples.size, accepted, acceptRate: accepted / samples.size };
    });
  eligible.sort((left, right) => right.acceptRate - left.acceptRate || right.n - left.n ||
    left.model.localeCompare(right.model) ||
    [...EFFORTS, null].indexOf(left.effort) - [...EFFORTS, null].indexOf(right.effort));
  return eligible[0] ?? null;
}

export function formatRecommendation(recommendation, taskClass) {
  if (!recommendation) return 'insufficient data\n';
  const { model, effort, acceptRate, n } = recommendation;
  return `${taskClass}: ${model} effort=${effort ?? '-'} accept-rate=${(acceptRate * 100).toFixed(1)}% n=${n}\n`;
}
