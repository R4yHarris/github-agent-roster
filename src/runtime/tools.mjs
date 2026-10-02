import { execFile } from 'node:child_process';
import { constants } from 'node:fs';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { redactSecrets } from './memory.mjs';
import { assertContractsInitialized, ContractsSubmoduleError, onlyMissingContractsScripts } from '../lib/contracts.mjs';
import { throwIfCancelled } from './cancel.mjs';
import { readTaskMetadata } from './estimate.mjs';

const execute = promisify(execFile);
const managedFiles = new Set(['assignment.md', 'task.md', 'recipe.yml', 'plan.md', 'context.md', 'research.md', 'result.md', 'review.md', 'estimate.md']);
export class ToolAccessError extends Error {
  code = 'ROSTER_TOOL_DENIED';
}
export const outsideWorktreeMessage = 'Refused: outside the worktree.';
export class OutsideWorktreeError extends ToolAccessError {
  constructor() { super(outsideWorktreeMessage); }
}
export const docsTestFiles = Object.freeze(['tests/repl.test.mjs']);
export const docsTestTimeoutMs = 60_000;
export const fullTestTimeoutMs = 300_000;

export function isOutsideWorktreePath(input) {
  if (typeof input !== 'string') return false;
  if (path.isAbsolute(input) || path.win32.isAbsolute(input) || /^[a-z]:/i.test(input)) return true;
  const parts = partsOf(input).filter((part) => part && part !== '.');
  return parts.includes('..') || parts[0] === 'vendor';
}

export function isReadmeOnlyScope(allowedFiles) {
  return Array.isArray(allowedFiles) && allowedFiles.length === 1 && allowedFiles[0] === 'README.md';
}

export function testCommandFor(allowedFiles) {
  return isReadmeOnlyScope(allowedFiles)
    ? { args: ['--test', ...docsTestFiles], timeoutMs: docsTestTimeoutMs, label: `node --test ${docsTestFiles.join(' ')}` }
    : { args: ['--test'], timeoutMs: fullTestTimeoutMs, label: 'node --test' };
}

export const plannerArtifactFiles = Object.freeze(['RECIPE.yml', 'TASK.md', 'ESTIMATE.md']);
export const planArtifactFiles = Object.freeze(['PLAN.md']);

function partsOf(file) {
  return file.replaceAll('\\', '/').toLowerCase().split('/');
}

function hasAmbiguousComponents(file) {
  return partsOf(file).some((part) => part !== '.' && part !== '..' &&
    /[:\x00-\x1f]|[. ]$/.test(part));
}

function isSecret(file) {
  const parts = partsOf(file);
  return parts.some((part, index) => part === '.env' || part.startsWith('.env.') ||
    part.endsWith('.env') || part.endsWith('.pem') ||
    (part === '.roster' && parts[index + 1] === 'vault'));
}

export function isForbiddenRead(file) {
  return isProtectedSurface(file);
}

function isProtectedSurface(file) {
  const parts = partsOf(file);
  return hasAmbiguousComponents(file) || isSecret(file) || isDebugLog(file) || isShellHistory(file) || isCheckpoint(file) || isRepoMap(file) ||
    parts.includes('.git') || parts.includes('agent-policy.yml') || parts[0] === 'vendor' ||
    parts.some((part, index) =>
      (part === '.github' && parts[index + 1] === 'workflows') ||
      (part === 'vendor' && parts[index + 1] === 'github-agent-contracts'));
}

export function isForbiddenWrite(file) {
  const parts = partsOf(file);
  return isProtectedSurface(file) || isManagedFile(file) ||
    parts.some((part, index) => part === '.roster' &&
      ['evals.jsonl', 'memory'].includes(parts[index + 1]));
}

export function isManagedFile(file) {
  const parts = partsOf(file);
  return parts.length === 1 && managedFiles.has(parts[0]) || isRunLog(file) || isDebugLog(file) || isShellHistory(file) || isCheckpoint(file) || isRepoMap(file);
}

export function isDebugLog(file) {
  const parts = partsOf(file);
  return parts[0] === '.roster' && parts[1] === 'logs';
}

export function isShellHistory(file) {
  const parts = partsOf(file);
  return parts[0] === '.roster' && (parts[1] === 'history' || parts[1]?.startsWith('history.'));
}

export function isCheckpoint(file) {
  const parts = partsOf(file);
  return parts[0] === '.roster' && parts[1] === 'checkpoints';
}

export function isRepoMap(file) {
  return partsOf(file).join('/') === '.roster/map.md';
}

export function isRunLog(file) {
  const parts = partsOf(file);
  return parts.length === 3 && parts[0] === '.roster' && parts[1] === 'runs' && parts[2].endsWith('.log');
}

export function isAllowedFile(file, allowedFiles) {
  const normalized = file.replaceAll('\\', '/');
  const candidate = process.platform === 'win32' ? normalized.toLowerCase() : normalized;
  return !isForbiddenWrite(normalized) && allowedFiles.some((pattern) => {
    const allowed = process.platform === 'win32' ? pattern.toLowerCase() : pattern;
    return allowed === '**/*' || candidate === allowed ||
      (allowed.endsWith('/**') && candidate.startsWith(allowed.slice(0, -2)));
  });
}

export function isRepairTestFile(file) {
  return typeof file === 'string' && !file.includes('\\') && !path.isAbsolute(file) && !path.win32.isAbsolute(file) &&
    !file.split('/').some((part) => !part || part === '.' || part === '..') &&
    /(?:^|\/)(?:test(?:[._-][^/]+)?|[^/]+[._-]test)\.[cm]?js$/.test(file) &&
    !isForbiddenWrite(file);
}

export function taskAndRepairFiles(allowedFiles, repairFiles = []) {
  if (!Array.isArray(repairFiles) || repairFiles.some((file) => !isRepairTestFile(file))) {
    throw new TypeError('Repair scope must contain only worktree-relative, unprotected test files');
  }
  return [...new Set([...allowedFiles, ...repairFiles])];
}

function failingTestPaths(output, root) {
  const files = new Set();
  let failed = false;
  for (const line of output.split(/\r?\n/)) {
    if (/^\s*(?:not )?ok\b/.test(line)) failed = /^\s*not ok\b/.test(line);
    const match = /^\s*test at (.+):\d+:\d+\s*$/.exec(line) ??
      (failed ? /^\s*location:\s*['"]?(.+?):\d+:\d+['"]?\s*$/.exec(line) : null);
    if (!match) continue;
    let candidate = match[1];
    if (candidate.startsWith('file:')) {
      try { candidate = fileURLToPath(candidate); }
      catch { continue; }
    }
    const file = path.relative(root, path.resolve(root, candidate)).split(path.sep).join('/');
    if (isRepairTestFile(file)) files.add(file);
  }
  return [...files];
}

function argumentsFor(value, required, optional = []) {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      required.some((key) => !Object.hasOwn(value, key)) ||
      Object.keys(value).some((key) => !required.includes(key) && !optional.includes(key))) {
    throw new TypeError(`Tool arguments must contain ${required.join(', ') || 'no fields'}`);
  }
}

function validateSearchTextArguments(args) {
  argumentsFor(args, ['query'], ['path']);
  if (typeof args.query !== 'string' || !args.query.length || /[\r\n\0]/.test(args.query)) {
    throw new ToolAccessError('search_text query must be nonempty, single-line literal text');
  }
}

export const toolDefinitions = [
  {
    type: 'function',
    function: {
      name: 'read_file',
      description: 'Read a UTF-8 file inside the worktree.',
      parameters: {
        type: 'object',
        properties: { path: { type: 'string' }, max_lines: { type: 'integer', minimum: 1 } },
        required: ['path'], additionalProperties: false,
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'write_file',
      description: 'Create or replace a UTF-8 file allowed by TASK.md inside the worktree.',
      parameters: {
        type: 'object',
        properties: { path: { type: 'string' }, content: { type: 'string' } },
        required: ['path', 'content'], additionalProperties: false,
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'list_dir',
      description: 'List entries in a worktree directory.',
      parameters: {
        type: 'object', properties: { path: { type: 'string' } },
        additionalProperties: false,
      },
    },
  },
  {
    type: 'function',
    function: {
      name: 'run_test',
      description: 'Run node --test in the worktree with a 60-second timeout.',
      parameters: { type: 'object', properties: {}, additionalProperties: false },
    },
  },
  {
    type: 'function',
    function: {
      name: 'search_text',
      description: 'Find literal text in regular worktree files; returns at most 50 matching lines.',
      parameters: {
        type: 'object',
        properties: { query: { type: 'string' }, path: { type: 'string' } },
        required: ['query'], additionalProperties: false,
      },
    },
  },
];

const writeDefinition = toolDefinitions.find(({ function: tool }) => tool.name === 'write_file');
export const plannerToolDefinitions = [{
  ...writeDefinition,
  function: {
    ...writeDefinition.function,
    description: 'Write a planning draft at RECIPE.yml, TASK.md, or ESTIMATE.md only. ' +
      'The harness validates the final task and finalizes these managed artifacts; no app-code writes.',
    parameters: {
      ...writeDefinition.function.parameters,
      properties: {
        path: { type: 'string', enum: [...plannerArtifactFiles] },
        content: { type: 'string', maxLength: 65_536 },
      },
    },
  },
}];

export async function createTools({
  worktree,
  allowedFiles,
  seat = 'coder',
  plannerArtifacts = plannerArtifactFiles,
  plannerReads = false,
  env = process.env,
  apiKeyEnv = 'ROSTER_API_KEY',
  memoryPath,
  allowRunTest = true,
  readmeOnlyDocs = false,
  sliceReadsOnly = false,
  runCommand = execute,
  onEvent, signal,
  beforeWrite,
  allowRepoMap = false,
} = {}) {
  if (!['planner', 'coder'].includes(seat)) throw new TypeError('Only planner and coder seats have file tools');
  if (seat === 'planner' && (!Array.isArray(plannerArtifacts) || !plannerArtifacts.length ||
      plannerArtifacts.some((name) => ![...plannerArtifactFiles, ...planArtifactFiles].includes(name)))) {
    throw new TypeError('Planner scope must contain only known root planning artifacts');
  }
  if (typeof allowRunTest !== 'boolean') throw new TypeError('run_test permission must be a boolean');
  if (typeof allowRepoMap !== 'boolean' || allowRepoMap && seat !== 'coder') throw new TypeError('Repo map access requires a coder policy');
  if (typeof plannerReads !== 'boolean' || plannerReads &&
      (seat !== 'planner' || plannerArtifacts.length !== 1 || plannerArtifacts[0] !== 'PLAN.md')) {
    throw new TypeError('Planner exploration requires PLAN.md-only write scope');
  }
  if (typeof sliceReadsOnly !== 'boolean' || sliceReadsOnly && seat !== 'coder') {
    throw new TypeError('Slice read scope requires a coder seat and a boolean permission');
  }
  if (typeof readmeOnlyDocs !== 'boolean' || readmeOnlyDocs &&
      (seat !== 'coder' || !Array.isArray(allowedFiles) || allowedFiles.length !== 1 || allowedFiles[0] !== 'README.md')) {
    throw new TypeError('README-only docs tools require coder scope limited to README.md');
  }
  if (onEvent !== undefined && typeof onEvent !== 'function') throw new TypeError('Live tool observer must be a function');
  if (beforeWrite !== undefined && typeof beforeWrite !== 'function') throw new TypeError('Pre-write checkpoint hook must be a function');
  const root = path.resolve(worktree);
  const status = await fs.lstat(root);
  if (!status.isDirectory() || status.isSymbolicLink()) {
    throw new Error('Worktree must be a real directory, not a symlink');
  }
  const canonicalRoot = await fs.realpath(root);
  if (allowRepoMap) {
    const file = path.join(root, 'TASK.md');
    const taskEntry = await fs.lstat(file);
    if (!taskEntry.isFile() || taskEntry.isSymbolicLink() || taskEntry.nlink !== 1 || taskEntry.size > 65536) {
      throw new Error('Repo map access requires a valid regular TASK.md');
    }
    const metadata = readTaskMetadata(await fs.readFile(file, 'utf8'));
    if (metadata.difficulty < 4 || metadata.task_class === 'docs') throw new Error('Repo map access requires difficulty 4+ and a non-docs task');
  }
  let readmeWritten = false;
  const repairFiles = new Set();
  const scopedFiles = () => taskAndRepairFiles(allowedFiles, [...repairFiles]);
  if (seat === 'coder' && (!Array.isArray(allowedFiles) || !allowedFiles.length)) {
    throw new TypeError('TASK.md must list files allowed for writing');
  }

  function readable(file, directory = false) {
    if (!sliceReadsOnly || file === 'TASK.md' || isAllowedFile(file, scopedFiles())) return true;
    if (!directory) return false;
    const candidate = process.platform === 'win32' ? file.toLowerCase() : file;
    return !candidate || scopedFiles().some((pattern) => {
      const allowed = process.platform === 'win32' ? pattern.toLowerCase() : pattern;
      return allowed.startsWith(`${candidate}/`);
    });
  }

  function locate(input, { directory = false, write = false } = {}) {
    throwIfCancelled(signal);
    if (isOutsideWorktreePath(input)) throw new OutsideWorktreeError();
    if (typeof input !== 'string' || !input.trim() || input.includes('\0') ||
        path.isAbsolute(input) || path.win32.isAbsolute(input)) {
      throw new ToolAccessError('Tool path must be relative to the worktree');
    }
    if (hasAmbiguousComponents(input)) {
      throw new ToolAccessError('Tool path must not contain ambiguous Windows components or alternate data streams');
    }
    const file = path.resolve(root, input);
    const relative = path.relative(root, file);
    if ((!directory && !relative) || relative === '..' ||
        relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
      throw new OutsideWorktreeError();
    }
    const normalized = relative.split(path.sep).join('/');
    const mapRead = !write && allowRepoMap && isRepoMap(normalized);
    if (plannerReads && partsOf(normalized).includes('.roster')) {
      throw new ToolAccessError('Plan exploration cannot read private .roster artifacts');
    }
    if (readmeOnlyDocs && !write && !['TASK.md', 'README.md'].includes(normalized) && !repairFiles.has(normalized)) {
      throw new ToolAccessError('README-only docs task may read only TASK.md and README.md; other paths are denied');
    }
    const allowed = seat === 'planner' ? plannerArtifacts.includes(input) : isAllowedFile(normalized, scopedFiles());
    if (write && (!allowed ||
        (memoryPath && path.relative(file, path.resolve(memoryPath)) === ''))) {
      throw new ToolAccessError(seat === 'planner'
        ? `Planner write_file allows only root ${plannerArtifacts.join(', ').replace(/, ([^,]+)$/, ', and $1')}`
        : `Writing ${normalized} is not allowed by TASK.md or worktree policy`);
    }
    if (isForbiddenRead(normalized) && !mapRead) {
      throw new ToolAccessError('Tool access to secrets, Git metadata, policy, workflows, contracts, or debug logs is refused');
    }
    if (!write && !mapRead && !readable(normalized, directory)) {
      throw new ToolAccessError(`Reading ${normalized} is not allowed by TASK.md slice scope`);
    }
    return { file, relative, normalized };
  }

  async function checkComponents(relative) {
    let current = root;
    const parts = relative.split(path.sep);
    for (const [index, part] of parts.entries()) {
      current = path.join(current, part);
      let entry;
      try {
        entry = await fs.lstat(current);
      } catch (error) {
        if (error.code === 'ENOENT') return;
        throw error;
      }
      if (entry.isSymbolicLink()) throw new ToolAccessError('Tool paths may not traverse symlinks');
      if (index < parts.length - 1 && !entry.isDirectory()) {
        throw new Error('A parent of the tool path is not a directory');
      }
    }
  }

  async function checkParent(file) {
    const canonicalParent = await fs.realpath(path.dirname(file));
    const relative = path.relative(canonicalRoot, canonicalParent);
    if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
      throw new OutsideWorktreeError();
    }
  }

  const plannerWrites = new Map();
  const tools = {
    async read_file(args) {
      argumentsFor(args, ['path'], ['max_lines']);
      if (args.max_lines !== undefined &&
          (!Number.isSafeInteger(args.max_lines) || args.max_lines < 1)) {
        throw new TypeError('read_file max_lines must be a positive safe integer');
      }
      const { file, relative } = locate(args.path);
      await checkComponents(relative);
      const entry = await fs.lstat(file);
      if (!entry.isFile()) throw new Error('read_file requires a regular file');
      if (isRepoMap(relative.split(path.sep).join('/')) && entry.nlink !== 1) throw new Error('Repo map must be a single-link file');
      await checkParent(file);
      const text = await fs.readFile(file, 'utf8');
      return args.max_lines === undefined ? text : text.split(/\r?\n/).slice(0, args.max_lines).join('\n');
    },

    async write_file(args) {
      argumentsFor(args, ['path', 'content']);
      if (typeof args.content !== 'string') throw new TypeError('write_file content must be text');
      if (seat === 'planner' && Buffer.byteLength(args.content, 'utf8') > 65_536) {
        throw new TypeError('Planner artifact content must be at most 64 KiB');
      }
      if (seat === 'planner' && redactSecrets(args.content, { env, apiKeyEnv }) !== args.content) {
        throw new Error('Planner artifacts must not contain credentials or private keys');
      }
      const { file, relative, normalized } = locate(args.path, { write: true });
      await checkComponents(relative);
      await checkComponents(relative);
      await fs.mkdir(path.dirname(file), { recursive: true });
      await checkComponents(relative);
      await checkParent(file);
      const existing = await fs.lstat(file).catch((error) => {
        if (error.code === 'ENOENT') return null;
        throw error;
      });
      if (existing && !existing.isFile()) throw new Error('write_file requires a regular file');
      if (seat === 'coder') await beforeWrite?.({ path: normalized, allowedFiles: scopedFiles() });
      const previous = plannerWrites.get(normalized);
      if (seat === 'planner' && (existing && !previous || !existing && previous)) {
        throw new Error('Planner cannot overwrite pre-existing or externally replaced artifacts');
      }
      const flags = seat === 'planner'
        ? constants.O_RDWR | (previous ? 0 : constants.O_CREAT | constants.O_EXCL)
        : constants.O_WRONLY | constants.O_CREAT | constants.O_TRUNC;
      const handle = await fs.open(file, flags | (constants.O_NOFOLLOW ?? 0), 0o600);
      try {
        if (seat === 'planner') {
          const current = await handle.stat();
          if (!current.isFile() || current.nlink !== 1 || previous &&
              (current.dev !== previous.dev || current.ino !== previous.ino || current.size !== previous.content.length)) {
            throw new Error('Planner artifact changed outside the planner writer');
          }
          if (previous) {
            const content = Buffer.alloc(previous.content.length);
            let offset = 0;
            while (offset < content.length) {
              const { bytesRead } = await handle.read(content, offset, content.length - offset, offset);
              if (!bytesRead) break;
              offset += bytesRead;
            }
            if (offset !== content.length || !content.equals(previous.content)) {
              throw new Error('Planner artifact changed outside the planner writer');
            }
            await handle.truncate(0);
          }
          await handle.writeFile(args.content, 'utf8');
          plannerWrites.set(normalized, { dev: current.dev, ino: current.ino, content: Buffer.from(args.content) });
        } else {
        await handle.writeFile(args.content, 'utf8');
        }
      } finally {
        await handle.close();
      }
      if (readmeOnlyDocs && normalized === 'README.md') readmeWritten = true;
      return { path: normalized, bytes: Buffer.byteLength(args.content, 'utf8') };
    },

    async list_dir(args = {}) {
      argumentsFor(args, [], ['path']);
      if (readmeOnlyDocs) throw new ToolAccessError('README-only docs task does not allow directory listing');
      const { file, relative, normalized } = locate(args.path ?? '.', { directory: true });
      if (relative) await checkComponents(relative);
      const entry = await fs.lstat(file);
      if (!entry.isDirectory()) throw new Error('list_dir requires a directory');
      if (relative) await checkParent(file);
      const entries = await fs.readdir(file, { withFileTypes: true });
      return entries.filter((item) => {
        const child = path.posix.join(normalized, item.name);
        return !isProtectedSurface(child) && (!plannerReads || !partsOf(child).includes('.roster')) &&
          readable(child, item.isDirectory());
      })
        .map((item) => ({
        name: item.name,
        type: item.isDirectory() ? 'directory' : item.isFile() ? 'file' :
          item.isSymbolicLink() ? 'symlink' : 'other',
        })).sort((left, right) => left.name.localeCompare(right.name));
    },

    async run_test(args = {}) {
      throwIfCancelled(signal);
      argumentsFor(args, []);
      if (readmeOnlyDocs && !readmeWritten) {
        throw new Error('README-only docs task must write README.md before running tests or other tools');
      }
      if (!allowRunTest) throw new ToolAccessError('run_test is disabled by tools.run_test');
      const testEnv = { ...env, ROSTER_SEAT: 'coder' };
      for (const name of [apiKeyEnv, 'GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH',
        'GH_TOKEN', 'GITHUB_TOKEN', 'NODE_TEST_CONTEXT']) delete testEnv[name];
      let command = testCommandFor(seat === 'coder' ? allowedFiles : []);
      if (command.args.length > 1) {
        const docsCheck = await fs.lstat(path.join(root, ...docsTestFiles[0].split('/'))).catch(() => null);
        if (!docsCheck?.isFile()) command = testCommandFor([]);
      }
      try {
        await assertContractsInitialized(root);
        const { stdout, stderr } = await runCommand(process.execPath, command.args, {
          cwd: root, timeout: command.timeoutMs, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024,
          env: testEnv, signal,
        });
        return { exit_code: 0, stdout, stderr };
      } catch (error) {
        throwIfCancelled(signal);
        if (error instanceof ContractsSubmoduleError) {
          await onEvent?.({ type: 'contracts-uninitialized' });
          throw error;
        }
        if (error.code === 'ETIMEDOUT' || error.killed) {
          throw new Error(`${command.label} timed out after ${command.timeoutMs / 1000} seconds`, { cause: error });
        }
        if (typeof error.code === 'number') {
          const result = { exit_code: error.code, stdout: error.stdout ?? '', stderr: error.stderr ?? '' };
          if (onlyMissingContractsScripts(result)) {
            await onEvent?.({ type: 'contracts-uninitialized' });
            throw new ContractsSubmoduleError({ cause: error, tests: { exit_code: result.exit_code } });
          }
          for (const file of failingTestPaths(`${result.stdout}\n${result.stderr}`, root)) {
            const target = path.join(root, file);
            await checkComponents(file.split('/').join(path.sep));
            await checkParent(target);
            const entry = await fs.lstat(target);
            if (!entry.isFile() || entry.nlink !== 1) throw new Error('Failing test repair requires a regular single-link file');
            repairFiles.add(file);
          }
          return { ...result, ...(repairFiles.size ? { repair_files: [...repairFiles] } : {}) };
        }
        throw new Error(`node --test could not run: ${error.message}`, { cause: error });
      }
    },

    async search_text(args) {
      validateSearchTextArguments(args);
      if (readmeOnlyDocs) {
        throw new ToolAccessError('README-only docs task does not allow repository search; read README.md directly');
      }
      const matches = [];
      async function visit(input) {
        const { file, relative, normalized } = locate(input, { directory: true });
        if (relative) await checkComponents(relative);
        const entry = await fs.lstat(file);
        if (entry.isDirectory()) {
          for (const child of await tools.list_dir({ path: input })) {
            if (child.type === 'directory' || child.type === 'file') {
              await visit(path.posix.join(normalized, child.name));
              if (matches.length > 50) return;
            }
          }
          return;
        }
        if (!entry.isFile()) throw new Error('search_text requires a regular file or directory');
        const text = await tools.read_file({ path: normalized });
        if (text.includes('\0')) return;
        for (const [index, line] of text.split(/\r?\n/).entries()) {
          if (line.includes(args.query)) {
            matches.push({ path: normalized, line: index + 1, text: line });
            if (matches.length > 50) return;
          }
        }
      }
      await visit(args.path ?? '.');
      return { matches: matches.slice(0, 50), truncated: matches.length > 50 };
    },
  };
  const guarded = Object.fromEntries(Object.entries(tools).map(([name, execute]) => [name, async (args) => {
    if (isOutsideWorktreePath(args?.path)) throw new OutsideWorktreeError();
    return execute(args);
  }]));
  const selected = seat === 'planner' ? plannerReads
    ? { read_file: guarded.read_file, list_dir: guarded.list_dir, search_text: guarded.search_text, write_file: guarded.write_file }
    : { write_file: guarded.write_file } : guarded;
  if (!onEvent) return selected;
  return Object.fromEntries(Object.entries(selected).map(([name, execute]) => [name, async (args) => {
    const location = args?.path ?? (['list_dir', 'search_text'].includes(name) ? '.' : undefined);
    if (isOutsideWorktreePath(location)) {
      await onEvent({ type: 'tool-refused', name });
      await onEvent({ type: 'tool-result', name, path: location, status: 'denied' });
      throw new OutsideWorktreeError();
    }
    if (name === 'search_text') {
      try {
        validateSearchTextArguments(args);
      } catch (error) {
        await onEvent({ type: 'tool-result', name, path: location, status: 'denied' });
        throw error;
      }
    }
    await onEvent({ type: 'tool', name, ...(location === undefined ? {} : { path: location }) });
    let result;
    try {
      result = await execute(args);
    } catch (error) {
      if (error?.code === 'ROSTER_RUN_LOG') throw error;
      await onEvent({ type: 'tool-result', name, ...(location === undefined ? {} : { path: location }),
        status: error instanceof ToolAccessError ? 'denied' : 'error',
        ...(error?.tests?.exit_code === undefined ? {} : { exit_code: error.tests.exit_code }) });
      throw error;
    }
    await onEvent({ type: 'tool-result', name, ...(location === undefined ? {} : { path: location }),
      status: 'ok', ...(result?.exit_code === undefined ? {} : { exit_code: result.exit_code }) });
    if (name === 'write_file' && [...plannerArtifactFiles, ...planArtifactFiles].includes(result.path)) {
      await onEvent({ type: 'wrote', path: result.path });
    }
    return result;
  }]));
}
