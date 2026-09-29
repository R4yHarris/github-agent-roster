import { promises as fs } from 'node:fs';
import path from 'node:path';
import { parseRecipe } from '../lib/recipe.mjs';
import { planAsk } from '../planner/stub.mjs';

export async function runPlanner({ worktree, issue, config, fetchImpl, env, vault }) {
  const plan = await planAsk(issue.body, {
    config, reference: `issue:${issue.number}`, title: issue.title, fetchImpl, env, vault,
  });
  const recipe = parseRecipe(plan.recipe);
  if (recipe.ask !== `issue:${issue.number}` || recipe.seats.length !== 2 ||
      recipe.seats[0].id !== 'planner' || recipe.seats[0].worker !== 'builtin' ||
      recipe.seats[1].id !== 'coder' || recipe.seats[1].worker !== 'builtin') {
    throw new Error('Builtin planner must emit exactly a planner seat followed by a coder seat');
  }
  const recipePath = path.join(worktree, 'RECIPE.yml');
  const taskPath = path.join(worktree, 'TASK.md');
  await fs.writeFile(recipePath, plan.recipe, { encoding: 'utf8', flag: 'wx' });
  await fs.writeFile(taskPath, plan.task, { encoding: 'utf8', flag: 'wx' });
  return { ...plan, recipePath, taskPath };
}
