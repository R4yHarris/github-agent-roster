import { taskFilesAllowed } from '../planner/stub.mjs';
import { withoutLlmKeys } from '../lib/config.mjs';
import { buildRun, mergeUsage } from '../metrics/run.mjs';
import { loadContext } from '../runtime/context.mjs';
import { estimateTask, readTaskMetadata } from '../runtime/estimate.mjs';
import { checkExcellence, redactEvidence, snapshotWorktree, taskSkipsTests, writeResult } from '../runtime/excellence.mjs';
import { runLoop } from '../runtime/loop.mjs';
import { appendMemory, coderMemoryRecord, seatMemoryPath } from '../runtime/memory.mjs';
import { runResearch } from '../runtime/research.mjs';
import { loadSkills, previewSkills } from '../runtime/skills.mjs';
import { createTools } from '../runtime/tools.mjs';
import { isLlmTimeout, retryCommandForTask } from '../llm/request.mjs';

export async function runCoder({
  worktree, repoRoot, config, task, session, fetchImpl, env = process.env, vault, runTestCommand,
  priorFeedback = null, onEvent, askKind,
  retryCommand = retryCommandForTask(task),
}) {
  const stages = [];
  const memoryPath = seatMemoryPath({
    repoRoot, memoryPath: config.paths.memory, seat: 'coder',
  });
  const changedFiles = new Set();
  let context;
  let research;
  let baseline;
  let verifiedSnapshot;
  let metadata;
  let tests;
  let result = {
    mode: config.llm.base_url ? 'llm' : 'stub',
    model: config.llm.base_url ? config.llm.model : 'builtin-stub',
    turns: 0, usage: null, response: null, summary: 'Coder preparation stopped before implementation.',
  };
  try {
    context = await loadContext({ worktree, memoryPath, repoRoot, config, env, priorFeedback, askKind });
    if (!context.minimalDocs) stages.push('principal');
    stages.push('context');
    const skipsTests = taskSkipsTests(context.task);
    if (config.llm.base_url && !skipsTests && config.tools?.run_test === false) {
      throw new Error('run_test is disabled by tools.run_test; enable it or explicitly declare TASK.md tests: none');
    }
    metadata = estimateTask(readTaskMetadata(context.task), [], config.llm.model || env?.ROSTER_MODEL || '');
    if (config.llm.base_url && !metadata.model) throw new Error('Set config.llm.model or TASK.md model for the coder seat');
    config = { ...config, llm: { ...config.llm, model: metadata.model } };
    result.model = config.llm.base_url ? metadata.model : 'builtin-stub';
    const tools = await createTools({
      worktree, allowedFiles: taskFilesAllowed(context.task), memoryPath,
      apiKeyEnv: config.llm.api_key_env, env: withoutLlmKeys(env, config), runCommand: runTestCommand,
      allowRunTest: config.tools?.run_test !== false,
      readmeOnlyDocs: context.contextPolicy.readmeOnlyDocs,
      onEvent,
    });
    if (!context.minimalDocs) {
      research = await runResearch({
        worktree, tools, expectedTask: context.task, config, fetchImpl, env, vault, onEvent, retryCommand,
      });
      stages.push('research');
      result.usage = research.usage;
      result.response = research.response;
    }
    const skills = await loadSkills({ repoRoot, skillsPath: config.paths.skills, task: context.task, names: context.skillNames });
    if (JSON.stringify(previewSkills(skills)) !== JSON.stringify(context.skills)) {
      throw new Error('Task skills changed after the context pack; refusing coder edits');
    }
    stages.push('skills');
    baseline = await snapshotWorktree(worktree, { memoryPath });
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
    result = await runLoop({
      config, context, tools: trackedTools, fetchImpl, env, vault, onEvent, retryCommand,
      verify: async (candidate) => {
        const evidence = await checkExcellence({
          worktree, task: context.task, result: candidate, baseline, memoryPath,
          env, apiKeyEnv: config.llm.api_key_env,
        });
        if (context.contextPolicy.readmeOnlyDocs && !changedFiles.has('README.md')) {
          evidence.pass = false;
          evidence.reasons.unshift('README-only docs task must write README.md before finishing');
        }
        if (evidence.pass) verifiedSnapshot = evidence.snapshot;
        return evidence;
      },
    });
    stages.push('tool_loop');
    result = { ...result, tests: result.tests ?? tests,
      response: result.response ?? research?.response ?? null,
      usage: research?.turns ? mergeUsage(research.usage, result.usage) : result.usage };
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    result = { ...result, error };
  }
  const timedOut = isLlmTimeout(result.error);
  result = { ...result, research, stages, ...(timedOut ? {
    timedOut: true,
    summary: 'Coder HTTP request timed out. No change was verified; this run did not complete.',
  } : {}) };
  const remember = (result, error) => appendMemory({
    file: memoryPath, repoRoot, env, apiKeyEnv: config.llm.api_key_env,
    record: coderMemoryRecord({
      task, session, mode: result?.mode, changedFiles: [...changedFiles].sort(), tests, error,
    }),
  });
  let memoryFailure;
  try {
    await remember(result, result.error);
    stages.push('memory');
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    memoryFailure = new Error(`Memory append failed: ${error.message}`, { cause: error });
    result.error ??= memoryFailure;
  }
  let excellence;
  try {
    excellence = context ? await checkExcellence({
      worktree, task: context.task, result, baseline, verifiedSnapshot, memoryPath,
      env, apiKeyEnv: config.llm.api_key_env,
    }) : { pass: false, reasons: [result.error.message], files: [], model: result.model, turns: result.turns };
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    excellence = { pass: false, reasons: [`Excellence inspection failed: ${error.message}`],
      files: [], model: result.model, turns: result.turns };
  }
  if (memoryFailure && result.error !== memoryFailure) excellence.reasons.push(memoryFailure.message);
  stages.push('excellence');
  let run = null;
  try {
    if (result.mode === 'llm') run = buildRun({
      config, response: result.response, task, session, env,
    });
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    excellence.pass = false;
    excellence.reasons.push(`AI-Run metadata failed: ${error.message}`);
  }
  const resultPath = await writeResult({
    worktree, result, excellence, run, env, apiKeyEnv: config.llm.api_key_env,
  });
  await onEvent?.({ type: 'wrote', path: 'RESULT.md' });
  stages.push('result');
  result = { ...result, excellence, resultPath, baseline, memoryPath, run, taskMetadata: metadata,
    contextPath: context?.contextPath, researchPath: research?.researchPath };
  if (!excellence.pass && (result.mode !== 'stub' || result.error)) {
    const failure = new Error(redactEvidence(excellence.reasons[0], {
      env, apiKeyEnv: config.llm.api_key_env,
    }), { cause: result.error });
    if (!result.error) await remember(result, failure);
    failure.result = result;
    throw failure;
  }
  return result;
}
