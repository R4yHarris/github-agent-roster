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
import { createTools, isAllowedFile, isManagedFile, isRepairTestFile, recipeAllowsTool, ToolAccessError } from '../runtime/tools.mjs';
import { statusSectionPresent } from '../runtime/readme-status.mjs';
import { isLlmTimeout, retryCommandForTask } from '../llm/request.mjs';
import { selectReasoning } from '../llm/reasoning.mjs';
import { throwIfCancelled } from '../runtime/cancel.mjs';
import { lstatSync } from 'node:fs';
import path from 'node:path';
import { captureCheckpoint } from '../lib/checkpoints.mjs';

export async function runCoder({
  worktree, repoRoot, config, task, session, fetchImpl, env = process.env, vault, runTestCommand,
  priorFeedback = null, onEvent, askKind,
  retryCommand = retryCommandForTask(task),
  signal,
  steeringControl,
  initialBaseline,
  initialScopeFiles = [],
  initialRepairFiles = [],
  continuation = null,
}) {
  const stages = [];
  const memoryPath = seatMemoryPath({
    repoRoot, memoryPath: config.paths.memory, seat: 'coder',
  });
  const changedFiles = new Set();
  const scopeFiles = new Set(initialScopeFiles ?? []);
  const repairFiles = new Set(initialRepairFiles ?? []);
  const withRepairs = (files) => [...new Set([...repairFiles, ...(files ?? [])])].sort();
  const scopeBlocked = new Set();
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
    throwIfCancelled(signal);
    context = await loadContext({ worktree, memoryPath, repoRoot, config, env, priorFeedback, askKind, continuation });
    if (!context.minimalDocs) stages.push('principal');
    stages.push('context');
    const skipsTests = taskSkipsTests(context.task);
    if (config.llm.base_url && !skipsTests && config.tools?.run_test === false) {
      throw new Error('run_test is disabled by tools.run_test; enable it or explicitly declare TASK.md tests: none');
    }
    const allowedFiles = taskFilesAllowed(context.task);
    metadata = estimateTask(readTaskMetadata(context.task), [], config.llm.model || env?.ROSTER_MODEL || '');
    // A harness reroute locks a new fleet model; TASK.md still names the planner's original route.
    if (config.llm.locked_model) metadata = { ...metadata, model: config.llm.locked_model };
    if (config.llm.base_url && !metadata.model) throw new Error('Set config.llm.model or TASK.md model for the coder seat');
    config = selectReasoning({ ...config, llm: { ...config.llm, model: metadata.model } },
      { kind: askKind ?? 'slice', taskClass: metadata.task_class, difficulty: metadata.difficulty });
    result.model = config.llm.base_url ? metadata.model : 'builtin-stub';
    const availableTools = await createTools({
      worktree, allowedFiles, memoryPath,
      apiKeyEnv: config.llm.api_key_env, env: withoutLlmKeys(env, config), runCommand: runTestCommand,
      allowRunTest: config.tools?.run_test !== false,
      allowInternet: config.tools?.internet === true,
      fetchImpl,
      readmeOnlyDocs: context.contextPolicy.readmeOnlyDocs,
      sliceReadsOnly: context.contextPolicy.sliceReadsOnly,
      allowRepoMap: context.contextPolicy.repoMap,
      scopeExpansion: config.seat.scope_expansion ?? 3,
      initialScopeFiles: [...scopeFiles],
      initialRepairFiles: [...repairFiles],
      beforeWrite: lstatSync(path.join(worktree, '.git'), { throwIfNoEntry: false })
        ? async ({ allowedFiles }) => {
          try {
            await captureCheckpoint({ worktree, task, allowedFiles, env,
              apiKeyEnv: config.llm.api_key_env, signal });
          } catch (error) {
            if (!(error instanceof Error)) throw error;
            await onEvent?.({ type: 'checkpoint', status: 'unavailable' });
          }
        } : undefined,
      signal,
      onEvent: async (event) => {
        if (event?.type === 'scope-limit') scopeBlocked.add(event.path);
        await onEvent?.(event);
      },
    });
    const tools = config.seat.recipe_tools === undefined ? availableTools : Object.fromEntries(
      Object.entries(availableTools).map(([name, execute]) => [name, async (args, options) => {
        if (!recipeAllowsTool(config.seat.recipe_tools, name)) {
          throw new ToolAccessError(`Seat coder tools allow-list denies ${name}`);
        }
        return execute(args, options);
      }]),
    );
    if (!context.minimalDocs) {
      research = await runResearch({
        worktree, tools, expectedTask: context.task, config, fetchImpl, env, vault, onEvent, retryCommand, signal,
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
    // A rerouted attempt keeps the first attempt's edits, so its diff is measured from the pre-coder state.
    if (initialBaseline) {
      for (const file of new Set([...initialBaseline.keys(), ...baseline.keys()])) {
        if (isManagedFile(file)) continue;
        if (initialBaseline.has(file)) baseline.set(file, initialBaseline.get(file));
        else baseline.delete(file);
      }
    }
    const track = (written) => {
      changedFiles.add(written.path);
      if (written.scope_expanded) scopeFiles.add(written.scope_path);
      else if (!isAllowedFile(written.path, allowedFiles) && isRepairTestFile(written.path)) repairFiles.add(written.path);
      return written;
    };
    const trackedTools = {
      ...tools,
      async write_file(args) {
        return track(await tools.write_file(args));
      },
      async edit_file(args) {
        return track(await tools.edit_file(args));
      },
      async delete_file(args) {
        const deleted = await tools.delete_file(args);
        changedFiles.add(deleted.path);
        return deleted;
      },
      async run_test(args, options) {
        tests = undefined;
        tests = await tools.run_test(args, options);
        return tests;
      },
    };
    result = await runLoop({
      config, context, tools: trackedTools, fetchImpl, env, vault, onEvent, retryCommand, signal, steeringControl,
      verify: async (candidate) => {
        const evidence = await checkExcellence({
          worktree, task: context.task, result: { ...candidate, scopeFiles: [...scopeFiles].sort(), repairFiles: withRepairs(candidate.repairFiles) }, baseline, memoryPath,
          env, apiKeyEnv: config.llm.api_key_env,
        });
        if (context.contextPolicy.readmeOnlyDocs && !changedFiles.has('README.md')) {
          const existing = await tools.read_file({ path: 'README.md' });
          if (!statusSectionPresent(existing)) {
            evidence.pass = false;
            evidence.reasons.unshift('README-only docs task must write README.md before finishing');
          }
        } else if (allowedFiles.length === 1 && !changedFiles.has(allowedFiles[0])) {
          evidence.pass = false;
          evidence.reasons.push(`Bounded task must write ${allowedFiles[0]} before finishing`);
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
  result = { ...result, research, stages, scopeFiles: [...scopeFiles].sort(), repairFiles: withRepairs(result.repairFiles),
    scopeBlocked: [...scopeBlocked].sort(), ...(timedOut ? {
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
    packBudgetChars: context?.packBudgetChars, priorFeedbackIncluded: context?.priorFeedbackIncluded,
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
