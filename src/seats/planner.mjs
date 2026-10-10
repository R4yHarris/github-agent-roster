import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify, TextDecoder } from 'node:util';
import { parseRecipe, RecipeError } from '../lib/recipe.mjs';
import { planAsk, planFromTask, runtimeRecipe as canonicalRecipe } from '../planner/stub.mjs';
import { appendMemory, readMemory, seatMemoryPath } from '../runtime/memory.mjs';
import { writeEstimate } from '../runtime/estimate.mjs';
import { buildRun } from '../metrics/run.mjs';
import { createTools, isForbiddenWrite, planArtifactFiles } from '../runtime/tools.mjs';
import { ensureLocalPath } from '../lib/paths.mjs';
import { outputDirectoryFromAsk, parseTaskDocument, taskFilesAllowed, taskSections } from '../planner/task.mjs';
import { deriveDesign, groundingDefinitions, groundingErrors, isCodeSlice, parseDesign, symbolIndex, withDesign }
  from '../planner/grounding.mjs';
import { validatePlanningReceipt } from '../planner/receipt.mjs';
import { askKinds, classifyAsk, clarificationHint } from '../planner/classify.mjs';
import { planOutline } from '../planner/plan.mjs';
import { relatedExports } from '../planner/related-exports.mjs';
import { retryCommandForTask } from '../llm/request.mjs';
import { selectReasoning } from '../llm/reasoning.mjs';
import { throwIfCancelled } from '../runtime/cancel.mjs';
import { planSlice, validatedPlanTask } from '../planner/plan-mode.mjs';
import { runtimeRecipe } from '../planner/stub.mjs';
import { critiquePlan } from '../planner/critic.mjs';
import { requireLifecycleHooks } from '../runtime/hooks.mjs';

export async function critiquePlannerHandoff(plan, {
  worktree, ask, title, reference, learningRoot, config, env, fetchImpl, vault, onEvent,
  retryCommand, signal, lockedModel, session, task,
}) {
  if (plan.error) return plan;
  await validateDirectoryHandoff(plan.task, ask, worktree, title);
  const initialPlannerArtifacts = {};
  for (const name of ['TASK.md', 'ESTIMATE.md']) {
    const content = await readArtifact(worktree, name);
    if (content !== null) initialPlannerArtifacts[name] = content;
  }
  if (initialPlannerArtifacts['TASK.md'] !== undefined && initialPlannerArtifacts['TASK.md'] !== plan.task) {
    throw new Error('TASK changed before plan critique');
  }
  const files = await trackedRepositoryFiles(worktree);
  if (!files) throw new Error('Plan critic could not inspect tracked repository files');
  const index = await symbolIndex(worktree, files);
  const testNames = [];
  for (const file of files.filter((file) => /(?:^tests\/|\.test\.|\.spec\.)/.test(file)).slice(0, 1500)) {
    const target = path.join(worktree, file);
    await ensureLocalPath(target, worktree);
    const entry = await fs.lstat(target);
    if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1 || entry.size > 256 * 1024) continue;
    const text = await fs.readFile(target, 'utf8');
    for (const match of text.matchAll(/\b(?:test|it)\s*\(\s*(['"])([^'"\n]+)\1/g)) testNames.push(match[2]);
  }
  const grounding = { index };
  let chat;
  const criticResponses = [];
  let criticConfig;
  if (config.planner?.critic_profile) {
    const { loadFleet, withFleetProfile } = await import('../lib/fleet.mjs');
    const { createBuiltinChat } = await import('../lib/llm.mjs');
    const fleet = await loadFleet({ cwd: learningRoot });
    const profile = fleet.profiles.find((entry) => entry.id === config.planner.critic_profile);
    if (!profile) throw new Error('Plan critic profile is not present in the fleet');
    if (profile.id === config.llm.fleet_profile ||
        profile.model === config.llm.model && profile.base_url === config.llm.base_url) {
      throw new Error('Plan critic requires a different fleet profile and endpoint/model pair from the planner');
    }
    const selected = withFleetProfile(config, profile);
    criticConfig = { ...selected, llm: { ...selected.llm, task_kind: undefined,
      task_class: undefined, max_tokens: 600, effort: 'none' } };
    const transport = createBuiltinChat(criticConfig, {
      fetchImpl, env, vault, signal, onEvent, retryCommand, retryLength: false,
    });
    chat = async (request) => {
      const result = await transport(request);
      criticResponses.push(transport.lastResponse);
      return result;
    };
  }
  let revision;
  const definitions = chat ? (await groundingDefinitions(worktree, files, `${title ?? ''}\n${ask}`)).text : undefined;
  const critic = await critiquePlan(plan.task, { index, testNames, signal, chat, definitions,
    revise: config.llm.base_url ? async ({ task, defects }) => {
      revision = await planAsk(ask, { config, reference, title, learningRoot, env, fetchImpl, vault,
        onEvent, retryCommand, signal, lockedModel, grounding,
        metadata: plan.metadata, criticFeedback: { task, defects },
      });
      if (revision.error) throw new Error(`Plan critic revision failed: ${revision.error}`);
      return ensureDesign(revision.task, grounding, `${title ?? ''}\n${ask}`);
    } : undefined,
  });
  critic.runs = criticResponses.map((response, index) => buildRun({ config: criticConfig, response, env: {},
    task, ...(session ? { session: `${session}-critic-${index + 1}` } : {}) }));
  if (revision?.response) critic.revisionRun = buildRun({ config, response: revision.response,
    task, ...(session ? { session: `${session}-revision` } : {}), env });
  await validateDirectoryHandoff(critic.task, ask, worktree, title);
  if (!critic.defects.length && !critic.revised) return { ...plan, critic };
  const tools = await createTools({ worktree, seat: 'planner', env, apiKeyEnv: config.llm.api_key_env, signal,
    initialPlannerArtifacts });
  const estimated = await writeEstimate(critic.task, {
    worktree, learningRoot, config, env, writeArtifact: tools.write_file,
  });
  await tools.write_file({ path: 'TASK.md', content: estimated.task });
  await onEvent?.({ type: 'plan-critic', defects: critic.defects.length, revised: critic.revised });
  return { ...plan, ...estimated, critic, ...(revision ? {
    turns: (plan.turns ?? 0) + (revision.turns ?? 0), response: revision.response,
  } : {}) };
}

const execFileAsync = promisify(execFile);
const repositoryFileLimit = 1500;

async function validateDirectoryHandoff(task, ask, worktree, title) {
  if (!outputDirectoryFromAsk(ask)) return;
  const document = planFromTask(task, ask, { issueTitle: title });
  for (const file of document.files_allowed) await ensureLocalPath(path.join(worktree, file), worktree);
}

// Tracked, writable paths ground planner-proposed child scope; protected and vendored paths never appear.
export async function trackedRepositoryFiles(worktree) {
  try {
    const { stdout } = await execFileAsync('git', ['ls-files', '-z'], { cwd: worktree, encoding: 'utf8',
      maxBuffer: 16 * 1024 * 1024, timeout: 30_000 });
    const files = stdout.split('\0').filter((file) => file && !file.startsWith('vendor/') && !isForbiddenWrite(file));
    return files.slice(0, repositoryFileLimit);
  } catch {
    return undefined;
  }
}

// Spec 5.3/§4.5: the slice planner sees real definitions for the Ask's nouns and cannot cite names that do not exist.
export async function sliceGrounding(worktree, askText) {
  const repositoryFiles = await trackedRepositoryFiles(worktree);
  if (!repositoryFiles?.length) return undefined;
  const [index, { related, text }] = await Promise.all([symbolIndex(worktree, repositoryFiles),
    groundingDefinitions(worktree, repositoryFiles, askText)]);
  return { index, related, definitions: text };
}

// Every planned code slice carries a Design; an invalid planner-written Design is replaced by a derived one.
export function ensureDesign(task, grounding, askText) {
  let files;
  try { files = taskFilesAllowed(task); } catch { return task; }
  if (!isCodeSlice(files)) return task;
  const written = parseDesign(task);
  const errors = written ? groundingErrors({ design: written, filesAllowed: files, askText, index: grounding.index }) : [];
  if (written && !errors.length) return task;
  const section = taskSections(task).sections.find(({ name }) => name === 'design');
  const stripped = section ? task.replace(section.source, '') : task;
  const derived = deriveDesign({ filesAllowed: files, index: grounding.index, related: grounding.related });
  return withDesign(stripped, errors.length ? { ...derived, rejected: errors[0] } : derived);
}

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
    await validateDirectoryHandoff(task, ask, worktree, issueTitle);
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

export async function acceptPlannerPlan({ worktree, ask, title, reference, learningRoot, config, env, signal, lockedModel }) {
  throwIfCancelled(signal);
  const source = await readArtifact(worktree, 'PLAN.md');
  if (source === null) throw new Error('An accepted plan requires PLAN.md in the prepared worktree');
  const task = validatedPlanTask(source, ask, title, lockedModel);
  const recipe = runtimeRecipe(reference);
  const tools = await createTools({ worktree, seat: 'planner', env, apiKeyEnv: config.llm.api_key_env, signal });
  await tools.write_file({ path: 'RECIPE.yml', content: recipe });
  await tools.write_file({ path: 'TASK.md', content: task });
  const estimate = await writeEstimate(task, { worktree, learningRoot, config, env, writeArtifact: tools.write_file });
  await tools.write_file({ path: 'TASK.md', content: estimate.task });
  return { task, recipe, ...estimate, recipePath: path.join(worktree, 'RECIPE.yml'),
    taskPath: path.join(worktree, 'TASK.md'), acceptedPlan: true, run: null, turns: 0, usage: null };
}

export async function runPlanner({
  worktree, repoRoot, issue, ask = issue?.body, title = issue?.title,
  reference = issue ? `issue:${issue.number}` : undefined, config, metadata, lockedModel,
  task = issue ? `issue-${issue.number}` : undefined,
  session = issue ? `roster-${issue.number}-planner` : undefined,
  fetchImpl, env, vault, learningRoot = repoRoot, onEvent, askKind,
  retryCommand = retryCommandForTask(task),
  signal,
  planMode = false,
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
  await requireLifecycleHooks('pre-plan', { worktree, env, apiKeyEnv: config.llm.api_key_env, signal, onEvent });
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
      signal, ...(planMode || kind !== 'slice' ? { plannerArtifacts: planArtifactFiles } : {}),
      plannerReads: planMode && kind === 'slice' });
    const options = { config, reference, title, fetchImpl, env, vault, onEvent, retryCommand, signal,
      onResponse: (response) => { lastResponse = response; } };
    if (planMode && kind === 'slice') {
      plan = await planSlice(ask, { ...options, metadata, lockedModel, tools });
      lastResponse = plan.response;
      await tools.write_file({ path: 'PLAN.md', content: plan.plan });
    } else if (kind === 'slice') {
      const grounding = config.llm.base_url ? await sliceGrounding(worktree, `${title ?? ''}\n${ask}`) : undefined;
      plan = await planAsk(ask, {
        ...options, memory, learningRoot, metadata, lockedModel, tools, grounding,
      });
      if (grounding && !plan.error) plan = { ...plan, task: ensureDesign(plan.task, grounding, `${title ?? ''}\n${ask}`) };
      await validateDirectoryHandoff(plan.task, ask, worktree, title);
      validateBuiltinRecipe(plan.recipe, reference);
      plan = { ...plan, ...await writeEstimate(plan.task, {
        worktree, learningRoot, config, env, recommendation: plan.feedback?.recommendation,
        writeArtifact: tools.write_file,
      }) };
      await tools.write_file({ path: 'RECIPE.yml', content: plan.recipe });
      await tools.write_file({ path: 'TASK.md', content: plan.task });
    } else {
      const repositoryFiles = config.llm.base_url ? await trackedRepositoryFiles(worktree) : undefined;
      plan = await planOutline(ask, { ...options, kind, repositoryFiles,
        existingExports: await relatedExports(worktree, repositoryFiles, `${title ?? ''}\n${ask}`) });
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
      : kind === 'slice' && !planMode ? 'Prepared RECIPE.yml and TASK.md' : `Prepared ${kind} PLAN.md; no implementation`,
    ...(plan.error ? { error: plan.error } : {}),
  } });
  const run = config.llm.base_url ? buildRun({ config, response: lastResponse, task, session, env }) : null;
  return { ...plan, askKind: kind, ...(kind === 'slice' && !planMode ? { recipePath, taskPath } : { planPath }), run };
}
