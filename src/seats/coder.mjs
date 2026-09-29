import { taskFilesAllowed } from '../planner/stub.mjs';
import { loadContext } from '../runtime/context.mjs';
import { runLoop } from '../runtime/loop.mjs';
import { appendMemory, coderMemoryRecord, seatMemoryPath } from '../runtime/memory.mjs';
import { createTools } from '../runtime/tools.mjs';
import { loadPrincipal } from './principal.mjs';

export async function runCoder({
  worktree, repoRoot, config, task, session, fetchImpl, env, vault, runTestCommand,
}) {
  const principal = await loadPrincipal({ repoRoot, id: config.seat.principal });
  const memoryPath = seatMemoryPath({
    repoRoot, memoryPath: config.paths.memory, seat: 'coder',
  });
  const context = await loadContext({ worktree, memoryPath, repoRoot, config, principal, env });
  const tools = await createTools({
    worktree, allowedFiles: taskFilesAllowed(context.task),
    apiKeyEnv: config.llm.api_key_env, env, runCommand: runTestCommand,
  });
  const changedFiles = new Set();
  let tests;
  const trackedTools = {
    ...tools,
    async write_file(args) {
      const written = await tools.write_file(args);
      changedFiles.add(written.path);
      return written;
    },
    async run_test(args) {
      tests = await tools.run_test(args);
      return tests;
    },
  };
  const remember = (result, error) => appendMemory({
    file: memoryPath, repoRoot, env, apiKeyEnv: config.llm.api_key_env,
    record: coderMemoryRecord({
      task, session, mode: result?.mode, changedFiles: [...changedFiles].sort(), tests, error,
    }),
  });
  let result;
  try {
    result = await runLoop({ config, context, tools: trackedTools, worktree, fetchImpl, env, vault });
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    await remember(undefined, error);
    throw error;
  }
  await remember(result);
  return result;
}
