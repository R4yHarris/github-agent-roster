import { taskFilesAllowed } from '../planner/stub.mjs';
import { loadContext } from '../runtime/context.mjs';
import { runLoop } from '../runtime/loop.mjs';
import { appendMemory, seatMemoryPath } from '../runtime/memory.mjs';
import { createTools } from '../runtime/tools.mjs';
import { loadPrincipal } from './principal.mjs';

export async function runCoder({
  worktree, repoRoot, config, task, session, fetchImpl, env, vault, runTestCommand,
}) {
  const principal = await loadPrincipal({ repoRoot, id: config.seat.principal });
  const memoryPath = seatMemoryPath({
    repoRoot, memoryPath: config.paths.memory, seat: 'coder',
  });
  const context = await loadContext({ worktree, memoryPath, repoRoot, config, principal });
  const tools = await createTools({
    worktree, allowedFiles: taskFilesAllowed(context.task),
    apiKeyEnv: config.llm.api_key_env, env, runCommand: runTestCommand,
  });
  let result;
  try {
    result = await runLoop({ config, context, tools, worktree, fetchImpl, env, vault });
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
