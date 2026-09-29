import { execFile } from 'node:child_process';
import { constants } from 'node:fs';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';

const execute = promisify(execFile);
const managedFiles = new Set(['assignment.md', 'task.md', 'recipe.yml', 'context.md', 'research.md', 'result.md', 'estimate.md']);

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
  return isProtectedSurface(file) || (parts.length === 1 && managedFiles.has(parts[0])) ||
    parts.some((part, index) => part === '.roster' && parts[index + 1] === 'evals.jsonl');
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

export async function createTools({
  worktree,
  allowedFiles,
  env = process.env,
  apiKeyEnv = 'ROSTER_API_KEY',
  runCommand = execute,
} = {}) {
  const root = path.resolve(worktree);
  const status = await fs.lstat(root);
  if (!status.isDirectory() || status.isSymbolicLink()) {
    throw new Error('Worktree must be a real directory, not a symlink');
  }
  const canonicalRoot = await fs.realpath(root);
  if (!Array.isArray(allowedFiles) || !allowedFiles.length) {
    throw new TypeError('TASK.md must list files allowed for writing');
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
    if (write && !isAllowedFile(normalized, allowedFiles)) {
      throw new Error(`Writing ${normalized} is not allowed by TASK.md or worktree policy`);
    }
    if (isForbiddenRead(normalized)) {
      throw new Error('Tool access to secrets, Git metadata, policy, workflows, or contracts is refused');
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
      const { file, relative, normalized } = locate(args.path, { write: true });
      await checkComponents(relative);
      await fs.mkdir(path.dirname(file), { recursive: true });
      await checkComponents(relative);
      await checkParent(file);
      const existing = await fs.lstat(file).catch((error) => {
        if (error.code === 'ENOENT') return null;
        throw error;
      });
      if (existing && !existing.isFile()) throw new Error('write_file requires a regular file');
      const handle = await fs.open(file, constants.O_WRONLY | constants.O_CREAT |
        constants.O_TRUNC | (constants.O_NOFOLLOW ?? 0), 0o600);
      try {
        await handle.writeFile(args.content, 'utf8');
      } finally {
        await handle.close();
      }
      return { path: normalized, bytes: Buffer.byteLength(args.content, 'utf8') };
    },

    async list_dir(args = {}) {
      argumentsFor(args, [], ['path']);
      const { file, relative, normalized } = locate(args.path ?? '.', { directory: true });
      if (relative) await checkComponents(relative);
      const entry = await fs.lstat(file);
      if (!entry.isDirectory()) throw new Error('list_dir requires a directory');
      if (relative) await checkParent(file);
      const entries = await fs.readdir(file, { withFileTypes: true });
      return entries.filter((item) => !isProtectedSurface(path.posix.join(normalized, item.name)))
        .map((item) => ({
        name: item.name,
        type: item.isDirectory() ? 'directory' : item.isFile() ? 'file' :
          item.isSymbolicLink() ? 'symlink' : 'other',
        })).sort((left, right) => left.name.localeCompare(right.name));
    },

    async run_test(args = {}) {
      argumentsFor(args, []);
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
  return tools;
}
