import path from 'node:path';
import { parseRecipe } from '../lib/recipe.mjs';
import { planAsk } from '../planner/stub.mjs';
import { appendMemory, readMemory, seatMemoryPath } from '../runtime/memory.mjs';
import { writeEstimate } from '../runtime/estimate.mjs';
import { buildRun } from '../metrics/run.mjs';
import { createTools } from '../runtime/tools.mjs';

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
  let lastResponse = null;
  const recipePath = path.join(worktree, 'RECIPE.yml');
  const taskPath = path.join(worktree, 'TASK.md');
  try {
    const tools = await createTools({ worktree, seat: 'planner', env, apiKeyEnv: config.llm.api_key_env });
    plan = await planAsk(ask, {
      config, reference, title, fetchImpl, env, vault, memory, learningRoot, metadata, lockedModel,
      tools,
      onResponse: (response) => { lastResponse = response; },
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
      writeArtifact: tools.write_file,
    }) };
    await tools.write_file({ path: 'RECIPE.yml', content: plan.recipe });
    await tools.write_file({ path: 'TASK.md', content: plan.task });
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
      : 'Prepared RECIPE.yml and TASK.md',
    ...(plan.error ? { error: plan.error } : {}),
  } });
  const run = config.llm.base_url ? buildRun({ config, response: lastResponse, task, session, env }) : null;
  return { ...plan, recipePath, taskPath, run };
}
