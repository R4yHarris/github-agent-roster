import { randomBytes } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { TextDecoder } from 'node:util';
import { cleanAskText, renderAsk } from '../planner/stub.mjs';
import { runCoder } from '../seats/coder.mjs';
import { runPlanner } from '../seats/planner.mjs';
import { loadConfig } from './config.mjs';
import { ensureLocalPath } from './paths.mjs';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));

async function readAsk(file) {
  const status = await fs.lstat(file);
  if (!status.isFile() || status.isSymbolicLink() || status.size > 16_384) {
    throw new Error('Ask file must be a regular UTF-8 file of at most 16 KiB');
  }
  let text;
  try {
    text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true })
      .decode(await fs.readFile(file));
  } catch {
    throw new Error('Ask file must be UTF-8');
  }
  return cleanAskText(text);
}

export async function runDemo({
  askFile,
  cwd = process.cwd(),
  repoRoot = rosterRoot,
  tempRoot = tmpdir(),
  config = loadConfig({ repoRoot }),
} = {}) {
  if (typeof askFile !== 'string' || !askFile.trim()) {
    throw new TypeError('Use roster run --ask-file PATH --runtime builtin');
  }
  const file = path.resolve(cwd, askFile);
  await ensureLocalPath(file, repoRoot);
  let ask = await readAsk(file);
  let title;
  if (file === path.resolve(repoRoot, 'templates', 'sdlc', 'ASK.md')) {
    if (!ask.includes('{{ASK}}')) throw new Error('Bundled Ask template has no {{ASK}} placeholder');
    const sample = await readAsk(path.join(repoRoot, 'fixtures', 'demo-task', 'ASK.md'));
    ask = renderAsk(sample);
    title = sample.split('\n')[0];
  } else {
    if (ask.includes('{{ASK}}')) throw new Error('Ask file contains an unresolved {{ASK}} placeholder');
    title = ask.split('\n').find((line) => line.trim() && !line.startsWith('#')) ?? path.basename(file);
  }
  const stubConfig = { ...config, llm: { ...config.llm, base_url: '', model: '', profile: '' } };
  const id = randomBytes(8).toString('hex');
  const task = `demo-${id}`;
  const worktreePath = await fs.mkdtemp(path.join(tempRoot, 'roster-demo-'));
  try {
    for (const name of ['AGENTS.md', 'README.md']) {
      const source = path.join(repoRoot, 'fixtures', 'demo-task', name);
      const status = await fs.lstat(source);
      if (!status.isFile() || status.isSymbolicLink()) {
        throw new Error(`Demo fixture ${name} must be a regular file`);
      }
      await fs.copyFile(source, path.join(worktreePath, name));
    }
    const planner = await runPlanner({
      worktree: worktreePath, repoRoot, ask, title, reference: `local:${task}`,
      task, session: `roster-${task}-planner`, config: stubConfig, env: {},
      fetchImpl: () => { throw new Error('Stub planner must not contact an LLM'); },
    });
    const coder = await runCoder({
      worktree: worktreePath, repoRoot, config: stubConfig, task,
      session: `roster-${task}-coder`, env: {},
      priorFeedback: planner.feedback?.context,
      fetchImpl: () => { throw new Error('Stub coder must not contact an LLM'); },
      runTestCommand: () => { throw new Error('Stub coder must not run tests'); },
    });
    return {
      worktreePath, recipePath: planner.recipePath, taskPath: planner.taskPath,
      resultPath: coder.resultPath, mode: coder.mode,
    };
  } catch (error) {
    await fs.rm(worktreePath, { recursive: true, force: true });
    throw error;
  }
}
