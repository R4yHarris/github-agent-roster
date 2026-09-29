import { randomUUID } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadConfig } from './config.mjs';
import { ensureLocalPath } from './paths.mjs';
import { planAsk, renderAsk } from '../planner/stub.mjs';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));

export async function writeAsk(ask, {
  repoRoot = rosterRoot, config = loadConfig({ repoRoot }), id = randomUUID(), fetchImpl, env, vault,
}) {
  if (typeof id !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(id)) {
    throw new TypeError('Ask ID must be an opaque local identifier');
  }
  const plan = await planAsk(ask, { config, reference: `local:${id}`, fetchImpl, env, vault });
  const directory = path.join(repoRoot, config.paths.asks);
  const draft = path.join(directory, id);
  const askPath = path.join(directory, `${id}.md`);
  const recipePath = path.join(draft, 'RECIPE.yml');
  const taskPath = path.join(draft, 'TASK.md');
  await ensureLocalPath(draft, repoRoot);
  await ensureLocalPath(askPath, repoRoot);
  await fs.mkdir(directory, { recursive: true });
  await fs.mkdir(draft);
  await ensureLocalPath(draft, repoRoot);
  await fs.writeFile(askPath, renderAsk(ask), { encoding: 'utf8', flag: 'wx' });
  await fs.writeFile(recipePath, plan.recipe, { encoding: 'utf8', flag: 'wx' });
  await fs.writeFile(taskPath, plan.task, { encoding: 'utf8', flag: 'wx' });
  return { id, askPath, recipePath, taskPath, usage: plan.usage };
}
