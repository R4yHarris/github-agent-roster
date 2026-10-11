import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify, TextDecoder } from 'node:util';
import { ensureLocalPath } from './paths.mjs';
import { classifyAsk } from '../planner/classify.mjs';
import { clarificationResult } from '../planner/clarify.mjs';
import { askRequirements, cleanAskText } from '../planner/stub.mjs';
import { redactEvidence } from '../runtime/excellence.mjs';

const execute = promisify(execFile);
const relativeFile = '.roster/asks/clarification.json';
const maxBytes = 65536;
const keys = ['version', 'id', 'originalAsk', 'interpretation', 'assumptions', 'questions', 'answer'];

function filename(repoRoot) {
  return path.join(repoRoot, '.roster', 'asks', 'clarification.json');
}

async function privatePath(repoRoot, env) {
  const file = filename(repoRoot);
  await ensureLocalPath(file, repoRoot);
  const marker = await fs.lstat(path.join(repoRoot, '.git')).catch((error) => {
    if (error.code === 'ENOENT') return null;
    throw error;
  });
  if (marker) {
    try {
      const { stdout } = await execute('git', ['ls-files', '--', relativeFile, relativeFile + '.*'], {
        cwd: repoRoot, env, encoding: 'utf8', timeout: 10000,
      });
      if (stdout.trim()) throw new Error('Tracked intake checkpoint');
      await Promise.all([relativeFile, relativeFile + '.tmp'].map((name) =>
        execute('git', ['check-ignore', '--quiet', '--', name], {
          cwd: repoRoot, env, encoding: 'utf8', timeout: 10000,
        })));
    } catch {
      throw new Error('Clarification storage must be untracked and gitignored; ignore .roster/asks/. No execution started.');
    }
  }
  return file;
}

function validate(record, env, apiKeyEnv) {
  if (!record || typeof record !== 'object' || Array.isArray(record) ||
      Object.keys(record).sort().join() !== [...keys].sort().join() || record.version !== 1 ||
      typeof record.id !== 'string' || !/^[a-f0-9-]{36}$/.test(record.id) ||
      typeof record.originalAsk !== 'string' || classifyAsk(record.originalAsk).kind !== 'clarify' ||
      cleanAskText(record.originalAsk) !== record.originalAsk ||
      redactEvidence(record.originalAsk, { env, apiKeyEnv }) !== record.originalAsk) {
    throw new Error('Invalid clarification checkpoint; no execution started.');
  }
  const expected = clarificationResult(record.originalAsk);
  for (const key of ['interpretation', 'assumptions', 'questions']) {
    if (JSON.stringify(record[key]) !== JSON.stringify(expected[key])) {
      throw new Error('Invalid clarification checkpoint guidance; no execution started.');
    }
  }
  if (record.answer !== null) boundedAnswer(record, record.answer, { env, apiKeyEnv });
  return record;
}

export async function readClarification({ repoRoot, env = process.env, apiKeyEnv }) {
  const file = await privatePath(repoRoot, env);
  let handle;
  try {
    handle = await fs.open(file, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw new Error('Clarification checkpoint unavailable; no execution started.', { cause: error });
  }
  try {
    const entry = await handle.stat();
    if (!entry.isFile() || entry.nlink !== 1 || entry.size > maxBytes ||
        (process.platform !== 'win32' && (entry.mode & 0o077))) {
      throw new Error('Clarification checkpoint must be a private, bounded regular single-link file; no execution started.');
    }
    let record;
    try {
      const bytes = await handle.readFile();
      if (bytes.length > maxBytes) throw new Error('Checkpoint exceeds the intake bound');
      record = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
      return validate(record, env, apiKeyEnv);
    } catch (error) {
      throw new Error('Invalid clarification checkpoint; cancel it explicitly before starting another. No execution started.', { cause: error });
    }
  } finally { await handle.close(); }
}

export async function saveClarification(ask, { repoRoot, env = process.env, apiKeyEnv }) {
  const originalAsk = cleanAskText(redactEvidence(cleanAskText(ask), { env, apiKeyEnv }));
  if (classifyAsk(originalAsk).kind !== 'clarify') throw new TypeError('Only ambiguous intake can create a clarification checkpoint');
  const previous = await readClarification({ repoRoot, env, apiKeyEnv });
  if (previous) {
    if (previous.originalAsk === originalAsk) return previous;
    throw new Error('A clarification checkpoint is already pending. Use /clarify or roster ask --pending; resume or cancel it before replacing it.');
  }
  const { interpretation, assumptions, questions } = clarificationResult(originalAsk);
  const record = { version: 1, id: randomUUID(), originalAsk, interpretation, assumptions, questions, answer: null };
  const file = await privatePath(repoRoot, env);
  await fs.mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
  await ensureLocalPath(file, repoRoot);
  await fs.writeFile(file, JSON.stringify(record) + '\n', { encoding: 'utf8', flag: 'wx', mode: 0o600 });
  return record;
}

export async function cancelClarification({ repoRoot, env = process.env, apiKeyEnv, id } = {}) {
  const file = await privatePath(repoRoot, env);
  if (id) {
    const record = await readClarification({ repoRoot, env });
    if (!record || record.id !== id) throw new Error('Clarification checkpoint changed; refusing to remove it.');
  }
  const entry = await fs.lstat(file).catch((error) => { if (error.code === 'ENOENT') return null; throw error; });
  if (!entry) return false;
  if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1) {
    throw new Error('Clarification checkpoint must be a regular single-link file before cancellation.');
  }
  await fs.unlink(file);
  return true;
}

function boundedAnswer(record, answer, options) {
  const text = cleanAskText(answer);
  if (redactEvidence(text, { env: options.env ?? process.env, apiKeyEnv: options.apiKeyEnv }) !== text) {
    throw new Error('Remove secrets from the clarification before resuming. No execution started.');
  }
  const scope = askRequirements(text);
  if (!scope.explicit && !scope.outputDirectory) {
    throw new TypeError('Clarification must explicitly declare Allowed Files or a relative output directory. The checkpoint remains pending.');
  }
  // The follow-up supplements original constraints; it never silently replaces them.
  const ask = cleanAskText(`${record.originalAsk}\n\n## Clarification\n${text}`);
  if (classifyAsk(ask).kind !== 'slice') {
    throw new TypeError('Clarification must describe one bounded slice. The checkpoint remains pending; cancel it to plan a new feature.');
  }
  return { ask, scope, text };
}

export async function resumeClarification(answer, options) {
  let record = await readClarification(options);
  if (!record) throw new Error('No pending clarification. Start a new Ask.');
  if (!answer && !record.answer) throw new Error('Answer the pending questions with explicit scope before continuing.');
  const { ask, scope, text } = boundedAnswer(record, answer ?? record.answer, options);
  if (record.answer !== text) {
    const file = await privatePath(options.repoRoot, options.env ?? process.env);
    const next = { ...record, id: randomUUID(), answer: text };
    const temporary = file + '.' + next.id;
    await ensureLocalPath(temporary, options.repoRoot);
    try {
      await fs.writeFile(temporary, JSON.stringify(next) + '\n', { encoding: 'utf8', flag: 'wx', mode: 0o600 });
      if (JSON.stringify(await readClarification(options)) !== JSON.stringify(record)) {
        throw new Error('Clarification checkpoint changed during resume; no execution started.');
      }
      await fs.rename(temporary, file);
      record = next;
    } finally { await fs.rm(temporary, { force: true }); }
  }
  return { record, ask, scope };
}

export function formatClarificationCheckpoint(record, { shell = false } = {}) {
  if (!record) return 'No pending clarification.\n';
  const guidance = clarificationResult(record.originalAsk).guidance;
  const staged = record.answer ? `Validated follow-up: ${record.answer}\nUse /clarify continue or roster ask --resume after restart.\n` : '';
  return `Pending clarification: ${record.id}\nOriginal Ask: ${record.originalAsk}\n${guidance}\n${staged}` +
    (shell ? 'Checkpoint saved privately. Use /clarify TEXT with explicit Allowed Files or a relative output directory to resume; /clarify cancel discards it.\n'
      : 'Checkpoint saved privately. Use roster ask --resume "TEXT" with explicit Allowed Files or a relative output directory to resume; roster ask --cancel discards it.\n');
}
