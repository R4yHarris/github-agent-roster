import path from 'node:path';
import { taskFilesAllowed } from '../planner/stub.mjs';
import { loadContext } from '../runtime/context.mjs';
import { runLoop } from '../runtime/loop.mjs';
import { appendMemory } from '../runtime/memory.mjs';
import { loadSkills } from '../runtime/skills.mjs';
import { createTools } from '../runtime/tools.mjs';

export async function runCoder({
  worktree, repoRoot, config, task, session, fetchImpl, env, runTestCommand,
}) {
  const memoryPath = path.join(repoRoot, config.paths.memory);
  const [context, skills] = await Promise.all([
    loadContext({ worktree, memoryPath, repoRoot }),
    loadSkills({ repoRoot, skillsPath: config.paths.skills }),
  ]);
  const tools = await createTools({
    worktree, allowedFiles: taskFilesAllowed(context.task),
    apiKeyEnv: config.llm.api_key_env, env, runCommand: runTestCommand,
  });
  let result;
  try {
    result = await runLoop({ config, context, skills, tools, worktree, fetchImpl, env });
  } catch (error) {
    await appendMemory({ file: memoryPath, repoRoot, record: {
      task, session, status: 'failed', error: error.message,
    } });
    throw error;
  }
  await appendMemory({ file: memoryPath, repoRoot, record: {
    task, session, status: result.mode, summary: result.summary,
  } });
  return result;
}
