import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { loadConfig } from './config.mjs';
import { githubRepository, renderIssueBody } from './issue.mjs';
import { ensureLocalPath } from './paths.mjs';
import { cleanAskText, planAsk } from '../planner/stub.mjs';
import { readTaskMetadata } from '../runtime/estimate.mjs';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));
const execFileAsync = promisify(execFile);

async function execute(program, args, cwd, env) {
  const { stdout } = await execFileAsync(program, args, {
    cwd, env, encoding: 'utf8', timeout: 30_000, maxBuffer: 1024 * 1024,
  });
  return stdout;
}

function quoteForShell(value) {
  if (process.platform === 'win32') return `'${value.replaceAll("'", "''")}'`;
  return `'${value.replaceAll("'", "'\"'\"'")}'`;
}

export async function writeAsk(ask, {
  repoRoot = rosterRoot, config = loadConfig({ repoRoot }), id = randomUUID(), fetchImpl, env, vault,
}) {
  if (typeof id !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(id)) {
    throw new TypeError('Ask ID must be an opaque local identifier');
  }
  const plan = await planAsk(ask, { config, reference: `local:${id}`, fetchImpl, env, vault, learningRoot: repoRoot });
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
  await fs.writeFile(askPath, renderIssueBody(ask, readTaskMetadata(plan.task)), {
    encoding: 'utf8', flag: 'wx',
  });
  await fs.writeFile(recipePath, plan.recipe, { encoding: 'utf8', flag: 'wx' });
  await fs.writeFile(taskPath, plan.task, { encoding: 'utf8', flag: 'wx' });
  return { id, askPath, recipePath, taskPath, usage: plan.usage };
}

export async function submitAsk(ask, {
  cwd = process.cwd(),
  repoRoot = rosterRoot,
  config = loadConfig({ repoRoot }),
  env = process.env,
  id = randomUUID(),
  runCommand = execute,
  fetchImpl,
  vault,
} = {}) {
  const text = cleanAskText(ask);
  const title = text.split('\n')[0];
  if (title.length > 240) throw new TypeError('Issue title must be at most 240 characters');
  const body = renderIssueBody(text);
  const commandEnv = { ...env, GH_PROMPT_DISABLED: '1' };
  try {
    await runCommand('gh', ['--version'], cwd, commandEnv);
  } catch (error) {
    if (error.code !== 'ENOENT') {
      throw new Error(`Could not check gh availability: ${error.message}`, { cause: error });
    }
    const offlineConfig = { ...config, llm: { ...config.llm, base_url: '', model: '' } };
    const draft = await writeAsk(text, { repoRoot, config: offlineConfig, env, id, fetchImpl, vault });
    return {
      mode: 'draft', ...draft,
      command: `gh issue create --title ${quoteForShell(title)} --body-file ${quoteForShell(draft.askPath)}`,
    };
  }
  const origin = await runCommand('git', ['remote', 'get-url', 'origin'], cwd, commandEnv);
  if (typeof origin !== 'string' || !origin.trim()) {
    throw new Error('git remote get-url origin returned no GitHub repository');
  }
  const repository = githubRepository(origin.trim());
  let response;
  try {
    response = await runCommand('gh', [
      'issue', 'create', '--repo', repository, '--title', title, '--body', body,
    ], cwd, commandEnv);
  } catch (error) {
    throw new Error(`gh issue create failed: ${error.message}`, { cause: error });
  }
  const prefix = `https://github.com/${repository}/issues/`;
  const url = typeof response === 'string' ? response.trim() : '';
  const number = url.slice(prefix.length);
  if (!url.toLowerCase().startsWith(prefix.toLowerCase()) ||
      !/^[1-9]\d*$/.test(number) || !Number.isSafeInteger(Number(number))) {
    throw new Error('gh issue create did not return an issue URL for the current repository');
  }
  return { mode: 'issue', url, number: Number(number), title };
}
