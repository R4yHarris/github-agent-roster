import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { inferTaskClass, joinLearning, loadLearning, TASK_CLASSES, validateLocalEvaluation } from '../lib/learn.mjs';

const fields = ['difficulty', 'estimate_min', 'task_class', 'model'];

export function estimateTask(metadata = {}, evaluations = [], defaultModel = '') {
  const { difficulty = 2, estimate_min = 15, task_class = 'feat', model = '' } = metadata;
  if (!Number.isInteger(difficulty) || difficulty < 1 || difficulty > 5) {
    throw new TypeError('Task difficulty must be an integer from 1 to 5');
  }
  if (!Number.isSafeInteger(estimate_min) || estimate_min < 0) {
    throw new TypeError('Task estimate_min must be a nonnegative integer');
  }
  if (!TASK_CLASSES.includes(task_class)) throw new TypeError('Task task_class must be feat, fix, docs, or test');
  const selectedModel = model || defaultModel;
  if (typeof model !== 'string' || typeof selectedModel !== 'string' ||
      (selectedModel && (!/^[A-Za-z0-9._:/-]+$/.test(selectedModel) ||
        ['unknown', 'builtin-stub'].includes(selectedModel)))) {
    throw new TypeError('Task model must be a served model id or empty');
  }
  const latest = new Map();
  for (const evaluation of evaluations) {
    validateLocalEvaluation(evaluation, 'Estimation history');
    const target = evaluation.sha ? `sha:${evaluation.sha.toLowerCase()}` : `session:${evaluation.session}`;
    latest.set(target, evaluation);
  }
  const samples = [...latest.values()].filter((evaluation) =>
    selectedModel && evaluation.model === selectedModel && evaluation.task_class === task_class &&
    evaluation.minutes != null);
  const accepted = samples.filter(({ verdict }) => verdict === 'accept')
    .map(({ minutes }) => minutes).sort((left, right) => left - right);
  const history = samples.length >= 3 && accepted.length > 0;
  const middle = Math.floor(accepted.length / 2);
  const median = !history ? estimate_min : accepted.length % 2 ? accepted[middle]
    : accepted[middle - 1] + (accepted[middle] - accepted[middle - 1]) / 2;
  return {
    difficulty, estimate_min: history ? Math.round(median) : estimate_min,
    task_class, model: selectedModel, source: history ? 'history' : 'task/default',
    n: samples.length, accepted: accepted.length,
  };
}

function readMetadata(header) {
  const metadata = {};
  for (const line of header.split('\n')) {
    const match = /^(difficulty|estimate_min|task_class|model):[ \t]*(.*)$/.exec(line);
    if (!match) continue;
    const [, field, value] = match;
    if (Object.hasOwn(metadata, field)) throw new TypeError(`TASK.md has duplicate ${field}`);
    if (['difficulty', 'estimate_min'].includes(field)) {
      if (!/^\d+$/.test(value)) throw new TypeError(`Task ${field} must be an integer`);
      metadata[field] = Number(value);
    } else metadata[field] = value;
  }
  metadata.task_class ??= inferTaskClass(header.replace(/^# Task: /, '').split('\n')[0]) ?? 'feat';
  return metadata;
}

function formatMetadata(metadata) {
  return fields.map((field) => `${field}: ${metadata[field]}`).join('\n');
}

export async function writeEstimate(task, {
  worktree, learningRoot, config, env = process.env,
}) {
  if (typeof task !== 'string' || !task.startsWith('# Task: ')) {
    throw new TypeError('Estimation requires a TASK.md document');
  }
  const normalized = task.replace(/\r\n/g, '\n');
  const section = normalized.search(/^## /m);
  const header = section < 0 ? normalized : normalized.slice(0, section);
  const body = section < 0 ? '' : normalized.slice(section);
  const { runs, evaluations } = loadLearning({ cwd: learningRoot });
  const standalone = evaluations.filter((evaluation) => !runs.some((run) =>
    (evaluation.sha && run.sha?.toLowerCase() === evaluation.sha.toLowerCase()) ||
    (evaluation.session && run.session === evaluation.session)));
  const joined = joinLearning([], runs, evaluations).filter(({ evaluation }) => evaluation)
    .map(({ model, task_class, task: runTask, evaluation }) => ({
      ...evaluation, model: model ?? evaluation.model,
      task_class: task_class ?? inferTaskClass(runTask) ?? evaluation.task_class,
    }));
  const metadata = estimateTask(readMetadata(header), [...standalone, ...joined],
    config.llm.model || env.ROSTER_MODEL || '');
  const cleanHeader = header.replace(/^(difficulty|estimate_min|task_class|model):[^\n]*\n?/gm, '').trimEnd();
  const updatedTask = `${cleanHeader}\n\n${formatMetadata(metadata)}\n\n${body}`;
  const estimate = `# Estimate\n\n${formatMetadata(metadata)}\n\n` +
    `Source: ${metadata.source}\nMatching timed evaluations: ${metadata.n}\n` +
    `Accepted timed evaluations: ${metadata.accepted}\n\n` +
    'A story-point style estimate, not a delivery promise. Compare with human-reported actuals.\n';
  const estimatePath = join(worktree, 'ESTIMATE.md');
  await fs.writeFile(estimatePath, estimate, { encoding: 'utf8', flag: 'wx' });
  return { task: updatedTask, metadata, estimate, estimatePath };
}
