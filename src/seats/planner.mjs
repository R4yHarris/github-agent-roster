import { promises as fs } from 'node:fs';
import path from 'node:path';
import { parseRecipe } from '../lib/recipe.mjs';
import { planAsk } from '../planner/stub.mjs';
import { appendMemory, readMemory, seatMemoryPath } from '../runtime/memory.mjs';

export async function runPlanner({
  worktree, repoRoot, issue, config, task = `issue-${issue.number}`,
  session = `roster-${issue.number}-planner`, fetchImpl, env, vault,
}) {
  if (typeof repoRoot !== 'string' || !repoRoot) {
    throw new TypeError('Planner requires the roster repository root for memory');
  }
  const memoryPath = seatMemoryPath({
    repoRoot, memoryPath: config.paths.memory, seat: 'planner',
  });
  const memory = await readMemory({ file: memoryPath, repoRoot, limit: 20 });
  let plan;
  const recipePath = path.join(worktree, 'RECIPE.yml');
  const taskPath = path.join(worktree, 'TASK.md');
  try {
    plan = await planAsk(issue.body, {
      config, reference: `issue:${issue.number}`, title: issue.title, fetchImpl, env, vault, memory,
    });
    const recipe = parseRecipe(plan.recipe);
    if (recipe.ask !== `issue:${issue.number}` || recipe.seats.length !== 2 ||
        recipe.seats[0].id !== 'planner' || recipe.seats[0].worker !== 'builtin' ||
        recipe.seats[1].id !== 'coder' || recipe.seats[1].worker !== 'builtin') {
      throw new Error('Builtin planner must emit exactly a planner seat followed by a coder seat');
    }
    await fs.writeFile(recipePath, plan.recipe, { encoding: 'utf8', flag: 'wx' });
    await fs.writeFile(taskPath, plan.task, { encoding: 'utf8', flag: 'wx' });
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    await appendMemory({ file: memoryPath, repoRoot, record: {
      task, session, status: 'failed', error: error.message,
    } });
    throw error;
  }
  await appendMemory({ file: memoryPath, repoRoot, record: {
    task, session, status: config.llm.base_url ? 'llm' : 'stub',
    summary: 'Prepared RECIPE.yml and TASK.md',
  } });
  return { ...plan, recipePath, taskPath };
}
