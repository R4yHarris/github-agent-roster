import { execFile } from 'node:child_process';
import { constants } from 'node:fs';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { redactSecrets } from './memory.mjs';

const execute = promisify(execFile);
const managedFiles = new Set(['assignment.md', 'task.md', 'recipe.yml', 'plan.md', 'context.md', 'research.md', 'result.md', 'review.md', 'estimate.md']);
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
  return hasAmbiguousComponents(file) || isSecret(file) ||
    parts.includes('.git') || parts.includes('agent-policy.yml') ||
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
  return parts.length === 1 && managedFiles.has(parts[0]) || isRunLog(file);
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

function argumentsFor(value, required, optional = []) {
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      required.some((key) => !Object.hasOwn(value, key)) ||
      Object.keys(value).some((key) => !required.includes(key) && !optional.includes(key))) {
    throw new TypeError(`Tool arguments must contain ${required.join(', ') || 'no fields'}`);
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
  env = process.env,
  apiKeyEnv = 'ROSTER_API_KEY',
  memoryPath,
  allowRunTest = true,
  readmeOnlyDocs = false,
  sliceReadsOnly = false,
  runCommand = execute,
  onEvent,
} = {}) {
  if (!['planner', 'coder'].includes(seat)) throw new TypeError('Only planner and coder seats have file tools');
  if (seat === 'planner' && (!Array.isArray(plannerArtifacts) || !plannerArtifacts.length ||
      plannerArtifacts.some((name) => ![...plannerArtifactFiles, ...planArtifactFiles].includes(name)))) {
    throw new TypeError('Planner scope must contain only known root planning artifacts');
  }
  if (typeof allowRunTest !== 'boolean') throw new TypeError('run_test permission must be a boolean');
  if (typeof sliceReadsOnly !== 'boolean' || sliceReadsOnly && seat !== 'coder') {
    throw new TypeError('Slice read scope requires a coder seat and a boolean permission');
  }
  if (typeof readmeOnlyDocs !== 'boolean' || readmeOnlyDocs &&
      (seat !== 'coder' || !Array.isArray(allowedFiles) || allowedFiles.length !== 1 || allowedFiles[0] !== 'README.md')) {
    throw new TypeError('README-only docs tools require coder scope limited to README.md');
  }
  if (onEvent !== undefined && typeof onEvent !== 'function') throw new TypeError('Live tool observer must be a function');
  const root = path.resolve(worktree);
  const status = await fs.lstat(root);
  if (!status.isDirectory() || status.isSymbolicLink()) {
    throw new Error('Worktree must be a real directory, not a symlink');
  }
  const canonicalRoot = await fs.realpath(root);
  let readmeWritten = false;
  if (seat === 'coder' && (!Array.isArray(allowedFiles) || !allowedFiles.length)) {
    throw new TypeError('TASK.md must list files allowed for writing');
  }

  function readable(file, directory = false) {
    if (!sliceReadsOnly || file === 'TASK.md' || isAllowedFile(file, allowedFiles)) return true;
    if (!directory) return false;
    const candidate = process.platform === 'win32' ? file.toLowerCase() : file;
    return !candidate || allowedFiles.some((pattern) => {
      const allowed = process.platform === 'win32' ? pattern.toLowerCase() : pattern;
      return allowed.startsWith(`${candidate}/`);
    });
  }

  function locate(input, { directory = false, write = false } = {}) {
    if (typeof input !== 'string' || !input.trim() || input.includes('\0') ||
        path.isAbsolute(input) || path.win32.isAbsolute(input)) {
      throw new Error('Tool path must be relative to the worktree');
    }
    if (hasAmbiguousComponents(input)) {
      throw new Error('Tool path must not contain ambiguous Windows components or alternate data streams');
    }
    const file = path.resolve(root, input);
    const relative = path.relative(root, file);
    if ((!directory && !relative) || relative === '..' ||
        relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
      throw new Error('Tool path must stay inside the worktree');
    }
    const normalized = relative.split(path.sep).join('/');
    if (readmeOnlyDocs && !write && !['TASK.md', 'README.md'].includes(normalized)) {
      throw new Error('README-only docs task may read only TASK.md and README.md; other paths are denied');
    }
    const allowed = seat === 'planner' ? plannerArtifacts.includes(input) : isAllowedFile(normalized, allowedFiles);
    if (write && (!allowed ||
        (memoryPath && path.relative(file, path.resolve(memoryPath)) === ''))) {
      throw new Error(seat === 'planner'
        ? `Planner write_file allows only root ${plannerArtifacts.join(', ').replace(/, ([^,]+)$/, ', and $1')}`
        : `Writing ${normalized} is not allowed by TASK.md or worktree policy`);
    }
    if (isForbiddenRead(normalized)) {
      throw new Error('Tool access to secrets, Git metadata, policy, workflows, or contracts is refused');
    }
    if (!write && !readable(normalized, directory)) {
      throw new Error(`Reading ${normalized} is not allowed by TASK.md slice scope`);
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
      if (entry.isSymbolicLink()) throw new Error('Tool paths may not traverse symlinks');
      if (index < parts.length - 1 && !entry.isDirectory()) {
        throw new Error('A parent of the tool path is not a directory');
      }
    }
  }

  async function checkParent(file) {
    const canonicalParent = await fs.realpath(path.dirname(file));
    const relative = path.relative(canonicalRoot, canonicalParent);
    if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
      throw new Error('Tool path resolves outside the worktree');
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
      if (readmeOnlyDocs) throw new Error('README-only docs task does not allow directory listing');
      const { file, relative, normalized } = locate(args.path ?? '.', { directory: true });
      if (relative) await checkComponents(relative);
      const entry = await fs.lstat(file);
      if (!entry.isDirectory()) throw new Error('list_dir requires a directory');
      if (relative) await checkParent(file);
      const entries = await fs.readdir(file, { withFileTypes: true });
      return entries.filter((item) => {
        const child = path.posix.join(normalized, item.name);
        return !isProtectedSurface(child) && readable(child, item.isDirectory());
      })
        .map((item) => ({
        name: item.name,
        type: item.isDirectory() ? 'directory' : item.isFile() ? 'file' :
          item.isSymbolicLink() ? 'symlink' : 'other',
        })).sort((left, right) => left.name.localeCompare(right.name));
    },

    async run_test(args = {}) {
      argumentsFor(args, []);
      if (readmeOnlyDocs && !readmeWritten) {
        throw new Error('README-only docs task must write README.md before running tests or other tools');
      }
      if (!allowRunTest) throw new Error('run_test is disabled by tools.run_test');
      const testEnv = { ...env, ROSTER_SEAT: 'coder' };
      for (const name of [apiKeyEnv, 'GITHUB_APP_ID', 'GITHUB_APP_PRIVATE_KEY_PATH',
        'GH_TOKEN', 'GITHUB_TOKEN', 'NODE_TEST_CONTEXT']) delete testEnv[name];
      try {
        const { stdout, stderr } = await runCommand(process.execPath, ['--test'], {
          cwd: root, timeout: 60_000, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024,
          env: testEnv,
        });
        return { exit_code: 0, stdout, stderr };
      } catch (error) {
        if (typeof error.code === 'number') {
          return { exit_code: error.code, stdout: error.stdout ?? '', stderr: error.stderr ?? '' };
        }
        if (error.code === 'ETIMEDOUT' || error.killed) {
          throw new Error('node --test timed out after 60 seconds', { cause: error });
        }
        throw new Error(`node --test could not run: ${error.message}`, { cause: error });
      }
    },

    async search_text(args) {
      argumentsFor(args, ['query'], ['path']);
      if (readmeOnlyDocs) {
        throw new Error('README-only docs task does not allow repository search; read README.md directly');
      }
      if (typeof args.query !== 'string' || !args.query.length || /[\r\n\0]/.test(args.query)) {
        throw new TypeError('search_text query must be nonempty, single-line literal text');
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
  const selected = seat === 'planner' ? { write_file: tools.write_file } : tools;
  if (!onEvent) return selected;
  return Object.fromEntries(Object.entries(selected).map(([name, execute]) => [name, async (args) => {
    const location = args?.path ?? (['list_dir', 'search_text'].includes(name) ? '.' : undefined);
    await onEvent({ type: 'tool', name, ...(location === undefined ? {} : { path: location }) });
    const result = await execute(args);
    if (name === 'write_file' && [...plannerArtifactFiles, ...planArtifactFiles].includes(result.path)) {
      await onEvent({ type: 'wrote', path: result.path });
    }
    return result;
  }]));
}
