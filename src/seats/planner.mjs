import { promises as fs } from 'node:fs';
import path from 'node:path';
import { TextDecoder } from 'node:util';
import { parseRecipe, RecipeError } from '../lib/recipe.mjs';
import { planAsk, planFromTask, runtimeRecipe as canonicalRecipe } from '../planner/stub.mjs';
import { appendMemory, readMemory, seatMemoryPath } from '../runtime/memory.mjs';
import { writeEstimate } from '../runtime/estimate.mjs';
import { buildRun } from '../metrics/run.mjs';
import { createTools, planArtifactFiles } from '../runtime/tools.mjs';
import { ensureLocalPath } from '../lib/paths.mjs';
import { parseTaskDocument, taskSections } from '../planner/task.mjs';
import { validatePlanningReceipt } from '../planner/receipt.mjs';
import { askKinds, classifyAsk, clarificationHint } from '../planner/classify.mjs';
import { planOutline } from '../planner/plan.mjs';
import { retryCommandForTask } from '../llm/request.mjs';
import { selectReasoning } from '../llm/reasoning.mjs';
import { throwIfCancelled } from '../runtime/cancel.mjs';

function validateBuiltinRecipe(source, reference) {
  const recipe = parseRecipe(source);
  if (recipe.ask !== reference || recipe.seats.length !== 3 ||
      recipe.seats[0].id !== 'planner' || recipe.seats[0].worker !== 'builtin' ||
      recipe.seats[1].id !== 'coder' || recipe.seats[1].worker !== 'builtin' ||
      recipe.seats[2].id !== 'reviewer' || recipe.seats[2].worker !== 'builtin') {
    throw new TypeError('Builtin planner must emit planner, coder, and reviewer seats for this issue in order');
  }
}

async function readArtifact(worktree, name) {
  const file = path.join(worktree, name);
  await ensureLocalPath(file, worktree);
  const entry = await fs.lstat(file).catch((error) => {
    if (error.code === 'ENOENT') return null;
    throw error;
  });
  if (!entry) return null;
  if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1 || entry.size > 65_536) {
    throw new Error('Existing planning artifacts must be regular, single-link files of at most 64 KiB');
  }
  return new TextDecoder('utf-8', { fatal: true }).decode(await fs.readFile(file));
}

export function readPlannerTask(worktree) {
  return readArtifact(worktree, 'TASK.md');
}

export function readPreviousReview(worktree) {
  return readArtifact(worktree, 'REVIEW.md');
}

export async function readPlannerHandoff({ worktree, reference, ask, issueTitle, issueBody, lockedModel }) {
  const [recipe, task] = await Promise.all([
    readArtifact(worktree, 'RECIPE.yml'), readArtifact(worktree, 'TASK.md'),
  ]);
  if (recipe === null || task === null) return { plan: null, reason: 'Existing RECIPE/TASK handoff is incomplete' };
  let runtimeRecipe = recipe;
  try {
    const metadata = planFromTask(task, ask, { issueTitle, issueBody });
    if (taskSections(task).sections.some(({ name }) => name === 'planning failure')) {
      throw new TypeError('Existing TASK records a failed planner attempt');
    }
    if (lockedModel && metadata.model && metadata.model !== lockedModel) {
      throw new TypeError('The routed planner must keep the selected fleet model');
    }
    try {
      validateBuiltinRecipe(recipe, reference);
    } catch (error) {
      if (!(error instanceof RecipeError)) throw error;
      const document = parseTaskDocument(task, { expectedAsk: ask, issueTitle, issueBody });
      validatePlanningReceipt(recipe, document);
      runtimeRecipe = canonicalRecipe(reference);
      validateBuiltinRecipe(runtimeRecipe, reference);
    }
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    return { plan: null, reason: 'Existing RECIPE/TASK do not validate for this issue; replanning is required' };
  }
  return { plan: { recipe: runtimeRecipe, normalizedRecipe: runtimeRecipe !== recipe, task, recipePath: path.join(worktree, 'RECIPE.yml'),
    taskPath: path.join(worktree, 'TASK.md'), reused: true, run: null, turns: 0, usage: null, response: null } };
}

export async function preparePlannerHandoff(plan, { worktree, learningRoot, config, env }) {
  const recipe = await readArtifact(worktree, 'RECIPE.yml');
  if ((plan.normalizedRecipe ? recipe !== null : recipe !== plan.recipe) ||
      await readArtifact(worktree, 'TASK.md') !== plan.task) {
    throw new Error('Existing RECIPE/TASK changed after validation; refusing the cached handoff');
  }
  if (plan.normalizedRecipe) {
    const tools = await createTools({ worktree, seat: 'planner', env, apiKeyEnv: config.llm.api_key_env });
    await tools.write_file({ path: 'RECIPE.yml', content: plan.recipe });
  }
  const { task: _updatedTask, ...estimated } = await writeEstimate(plan.task, { worktree, learningRoot, config, env });
  return { ...plan, ...estimated };
}

export async function runPlanner({
  worktree, repoRoot, issue, ask = issue?.body, title = issue?.title,
  reference = issue ? `issue:${issue.number}` : undefined, config, metadata, lockedModel,
  task = issue ? `issue-${issue.number}` : undefined,
  session = issue ? `roster-${issue.number}-planner` : undefined,
  fetchImpl, env, vault, learningRoot = repoRoot, onEvent, askKind,
  retryCommand = retryCommandForTask(task),
  signal,
}) {
  throwIfCancelled(signal);
  if (typeof repoRoot !== 'string' || !repoRoot) {
    throw new TypeError('Planner requires the roster repository root for memory');
  }
  if (typeof ask !== 'string' || typeof reference !== 'string' ||
      typeof task !== 'string' || typeof session !== 'string') {
    throw new TypeError('Planner requires an issue or a complete local Ask assignment');
  }
  const kind = askKind ?? classifyAsk(ask, { title }).kind;
  if (!askKinds.includes(kind)) throw new TypeError('Unknown Ask kind');
  if (kind === 'clarify') throw new TypeError(clarificationHint);
  config = selectReasoning(config, { kind, taskClass: metadata?.task_class, difficulty: metadata?.difficulty });
  const memoryPath = seatMemoryPath({
    repoRoot, memoryPath: config.paths.memory, seat: 'planner',
  });
  const memory = await readMemory({
    file: memoryPath, repoRoot, limit: 20, env, apiKeyEnv: config.llm.api_key_env,
  });
  let plan;
  let lastResponse = null;
  const recipePath = path.join(worktree, 'RECIPE.yml');
  const taskPath = path.join(worktree, 'TASK.md');
  const planPath = path.join(worktree, 'PLAN.md');
  try {
    const tools = await createTools({ worktree, seat: 'planner', env, apiKeyEnv: config.llm.api_key_env, onEvent,
      signal, ...(kind === 'slice' ? {} : { plannerArtifacts: planArtifactFiles }) });
    const options = { config, reference, title, fetchImpl, env, vault, onEvent, retryCommand, signal,
      onResponse: (response) => { lastResponse = response; } };
    if (kind === 'slice') {
      plan = await planAsk(ask, {
        ...options, memory, learningRoot, metadata, lockedModel, tools,
      });
      validateBuiltinRecipe(plan.recipe, reference);
      plan = { ...plan, ...await writeEstimate(plan.task, {
        worktree, learningRoot, config, env, recommendation: plan.feedback?.recommendation,
        writeArtifact: tools.write_file,
      }) };
      await tools.write_file({ path: 'RECIPE.yml', content: plan.recipe });
      await tools.write_file({ path: 'TASK.md', content: plan.task });
    } else {
      plan = await planOutline(ask, { ...options, kind });
      await tools.write_file({ path: 'PLAN.md', content: plan.plan });
    }
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    if (config.llm.base_url && lastResponse) {
      error.run = buildRun({ config, response: lastResponse, task, session, env });
    }
    await appendMemory({ file: memoryPath, repoRoot, env, apiKeyEnv: config.llm.api_key_env, record: {
      task, session, status: 'failed', error: error.message,
    } });
    throw error;
  }
  await appendMemory({ file: memoryPath, repoRoot, env, apiKeyEnv: config.llm.api_key_env, record: {
    task, session, status: plan.error ? 'failed' : config.llm.base_url ? 'llm' : 'stub',
    summary: plan.error ? 'Prepared unverified RECIPE.yml and TASK.md stubs after planning failure'
      : kind === 'slice' ? 'Prepared RECIPE.yml and TASK.md' : `Prepared ${kind} PLAN.md; no implementation`,
    ...(plan.error ? { error: plan.error } : {}),
  } });
  const run = config.llm.base_url ? buildRun({ config, response: lastResponse, task, session, env }) : null;
  return { ...plan, askKind: kind, ...(kind === 'slice' ? { recipePath, taskPath } : { planPath }), run };
}
