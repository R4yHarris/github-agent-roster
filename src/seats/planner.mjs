import { promises as fs } from 'node:fs';
import path from 'node:path';
import { parseRecipe } from '../lib/recipe.mjs';
import { planAsk } from '../planner/stub.mjs';
import { appendMemory, readMemory, seatMemoryPath } from '../runtime/memory.mjs';
import { writeEstimate } from '../runtime/estimate.mjs';

export async function runPlanner({
  worktree, repoRoot, issue, ask = issue?.body, title = issue?.title,
  reference = issue ? `issue:${issue.number}` : undefined, config, metadata, lockedModel,
  task = issue ? `issue-${issue.number}` : undefined,
  session = issue ? `roster-${issue.number}-planner` : undefined,
  fetchImpl, env, vault, learningRoot = repoRoot,
}) {
  if (typeof repoRoot !== 'string' || !repoRoot) {
    throw new TypeError('Planner requires the roster repository root for memory');
  }
  if (typeof ask !== 'string' || typeof reference !== 'string' ||
      typeof task !== 'string' || typeof session !== 'string') {
    throw new TypeError('Planner requires an issue or a complete local Ask assignment');
  }
  const memoryPath = seatMemoryPath({
    repoRoot, memoryPath: config.paths.memory, seat: 'planner',
  });
  const memory = await readMemory({
    file: memoryPath, repoRoot, limit: 20, env, apiKeyEnv: config.llm.api_key_env,
  });
  let plan;
  const recipePath = path.join(worktree, 'RECIPE.yml');
  const taskPath = path.join(worktree, 'TASK.md');
  try {
    plan = await planAsk(ask, {
      config, reference, title, fetchImpl, env, vault, memory, learningRoot, metadata, lockedModel,
    });
    const recipe = parseRecipe(plan.recipe);
    if (recipe.ask !== reference || recipe.seats.length !== 3 ||
        recipe.seats[0].id !== 'planner' || recipe.seats[0].worker !== 'builtin' ||
        recipe.seats[1].id !== 'coder' || recipe.seats[1].worker !== 'builtin' ||
        recipe.seats[2].id !== 'reviewer' || recipe.seats[2].worker !== 'builtin') {
      throw new Error('Builtin planner must emit planner, coder, and reviewer seats in order');
    }
    plan = { ...plan, ...await writeEstimate(plan.task, {
      worktree, learningRoot, config, env, recommendation: plan.feedback?.recommendation,
    }) };
    await fs.writeFile(recipePath, plan.recipe, { encoding: 'utf8', flag: 'wx' });
    await fs.writeFile(taskPath, plan.task, { encoding: 'utf8', flag: 'wx' });
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    await appendMemory({ file: memoryPath, repoRoot, env, apiKeyEnv: config.llm.api_key_env, record: {
      task, session, status: 'failed', error: error.message,
    } });
    throw error;
  }
  await appendMemory({ file: memoryPath, repoRoot, env, apiKeyEnv: config.llm.api_key_env, record: {
    task, session, status: config.llm.base_url ? 'llm' : 'stub',
    summary: 'Prepared RECIPE.yml and TASK.md',
  } });
  return { ...plan, recipePath, taskPath };
}
