import { taskFilesAllowed } from '../planner/stub.mjs';
import { buildRun, mergeUsage } from '../metrics/run.mjs';
import { loadContext } from '../runtime/context.mjs';
import { checkExcellence, redactEvidence, snapshotWorktree, writeResult } from '../runtime/excellence.mjs';
import { runLoop } from '../runtime/loop.mjs';
import { appendMemory, coderMemoryRecord, seatMemoryPath } from '../runtime/memory.mjs';
import { runResearch } from '../runtime/research.mjs';
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
  const research = await runResearch({
    worktree, tools, expectedTask: context.task, config, fetchImpl, env, vault,
  });
  const baseline = await snapshotWorktree(worktree);
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
      tests = undefined;
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
    result = { ...result, research,
      tests: result.tests ?? tests,
      usage: research.turns ? mergeUsage(research.usage, result.usage) : result.usage };
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    await remember(undefined, error);
    throw error;
  }
  await remember(result, result.error);
  let excellence;
  try {
    excellence = await checkExcellence({
      worktree, task: context.task, result, baseline, env, apiKeyEnv: config.llm.api_key_env,
    });
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    excellence = { pass: false, reasons: [`Excellence inspection failed: ${error.message}`],
      files: [], model: result.model, turns: result.turns };
  }
  let run = null;
  try {
    if (result.mode === 'llm') run = buildRun({
      config, usage: result.usage ?? {}, task, session, env,
    });
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    excellence.pass = false;
    excellence.reasons.push(`AI-Run metadata failed: ${error.message}`);
  }
  const resultPath = await writeResult({
    worktree, result, excellence, run, env, apiKeyEnv: config.llm.api_key_env,
  });
  result = { ...result, excellence, resultPath, baseline };
  if (!excellence.pass && result.mode !== 'stub') {
    const failure = new Error(redactEvidence(excellence.reasons[0], {
      env, apiKeyEnv: config.llm.api_key_env,
    }), { cause: result.error });
    if (!result.error) await remember(result, failure);
    failure.result = result;
    throw failure;
  }
  return result;
}
