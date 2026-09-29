import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { excellenceFailed, inferTaskClass, joinLearning, loadLearning, median, TASK_CLASSES, validateLocalEvaluation } from '../lib/learn.mjs';
import { splitTaskFrontmatter } from './skills.mjs';

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
  const sessionShas = new Map();
  for (const evaluation of evaluations) {
    validateLocalEvaluation(evaluation, 'Estimation history');
    if (evaluation.sha && evaluation.session) sessionShas.set(evaluation.session, evaluation.sha.toLowerCase());
  }
  for (const evaluation of evaluations) {
    const sha = evaluation.sha?.toLowerCase() ?? sessionShas.get(evaluation.session);
    const target = sha ? `sha:${sha}` : `session:${evaluation.session}`;
    latest.set(target, evaluation);
  }
  const samples = [...latest.values()].filter((evaluation) =>
    selectedModel && evaluation.model === selectedModel && evaluation.task_class === task_class &&
    evaluation.minutes != null);
  const accepted = samples.filter((evaluation) => evaluation.verdict === 'accept' && !excellenceFailed(evaluation))
    .map(({ minutes }) => minutes);
  const history = samples.length >= 3 && accepted.length > 0;
  return {
    difficulty, estimate_min: history ? Math.round(median(accepted)) : estimate_min,
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
  worktree, learningRoot, config, env = process.env, recommendation = null,
}) {
  const taskMetadata = readTaskMetadata(task);
  const { runs, evaluations } = loadLearning({ cwd: learningRoot });
  const standalone = evaluations.filter((evaluation) => !runs.some((run) =>
    (evaluation.sha && run.sha?.toLowerCase() === evaluation.sha.toLowerCase()) ||
    (evaluation.session && run.session === evaluation.session)));
  const joined = joinLearning([], runs, evaluations).filter(({ evaluation }) => evaluation)
    .map(({ model, task_class, task: runTask, excellence, evaluation }) => ({
      ...evaluation, model: model ?? evaluation.model,
      ...(excellence === undefined ? {} : { excellence }),
      task_class: task_class ?? inferTaskClass(runTask) ?? evaluation.task_class,
    }));
  const metadata = estimateTask(taskMetadata, [...standalone, ...joined],
    config.llm.model || env.ROSTER_MODEL || '');
  if (recommendation?.estimate_min != null) {
    if (recommendation.model !== metadata.model || !Number.isSafeInteger(recommendation.n) || recommendation.n < 3) {
      throw new Error('Recommended estimate must match the selected model with at least three samples');
    }
    estimateTask({ ...metadata, estimate_min: recommendation.estimate_min });
    Object.assign(metadata, { estimate_min: recommendation.estimate_min, source: 'recommendation',
      n: recommendation.n, accepted: recommendation.accepted });
  }
  const updatedTask = updateTaskMetadata(task, metadata);
  const evidence = metadata.source === 'recommendation'
    ? `Recommendation samples: ${metadata.n}\nAccepted samples: ${metadata.accepted}\n`
    : `Matching timed evaluations: ${metadata.n}\nAccepted timed evaluations: ${metadata.accepted}\n`;
  const estimate = `# Estimate\n\n${formatMetadata(metadata)}\n\nSource: ${metadata.source}\n${evidence}\n` +
    'A story-point style estimate, not a delivery promise. Compare with human-reported actuals.\n';
  const estimatePath = join(worktree, 'ESTIMATE.md');
  await fs.writeFile(estimatePath, estimate, { encoding: 'utf8', flag: 'wx' });
  return { task: updatedTask, metadata, estimate, estimatePath };
}

function taskParts(task) {
  if (typeof task !== 'string') {
    throw new TypeError('Estimation requires a TASK.md document');
  }
  const { frontmatter, body: normalized } = splitTaskFrontmatter(task);
  if (!normalized.startsWith('# Task: ')) {
    throw new TypeError('Estimation requires a TASK.md document');
  }
  const section = normalized.search(/^## /m);
  const header = section < 0 ? normalized : normalized.slice(0, section);
  const body = section < 0 ? '' : normalized.slice(section);
  return { frontmatter, header, body };
}

export function readTaskMetadata(task) {
  return estimateTask(readMetadata(taskParts(task).header));
}

export function updateTaskMetadata(task, metadata) {
  const { frontmatter, header, body } = taskParts(task);
  const values = estimateTask({ ...readMetadata(header), ...metadata });
  const cleanHeader = header.replace(/^(difficulty|estimate_min|task_class|model):[^\n]*\n?/gm, '').trimEnd();
  return `${frontmatter}${cleanHeader}\n\n${formatMetadata(values)}\n\n${body}`;
}
