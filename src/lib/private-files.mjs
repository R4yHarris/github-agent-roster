import { execFile } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { promises as fs, lstatSync } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { readConfigFile } from './config.mjs';
import { ensureLocalPath } from './paths.mjs';

const execute = promisify(execFile);

function privatePath(repoRoot, name) {
  if (!['config.yml', 'fleet.yml', 'capabilities.yml'].includes(name)) {
    throw new TypeError('Unsupported private settings filename');
  }
  return path.join(repoRoot, '.roster', name);
}

export async function readPrivateFile(repoRoot, name) {
  const file = privatePath(repoRoot, name);
  await ensureLocalPath(file, repoRoot);
  try {
    return readConfigFile(file);
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

export async function ensurePrivateFilesIgnored(repoRoot, names) {
  names.forEach((name) => privatePath(repoRoot, name));
  const ignoreFile = path.join(repoRoot, '.gitignore');
  await ensureLocalPath(ignoreFile, repoRoot);
  let source = '';
  try {
    const entry = await fs.lstat(ignoreFile);
    if (!entry.isFile() || entry.isSymbolicLink()) throw new Error('.gitignore must be a regular file');
    source = await fs.readFile(ignoreFile, 'utf8');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  const patterns = names.flatMap((name) => [`.roster/${name}`, `.roster/${name}.*`]);
  let ignored = false;
  if (lstatSync(path.join(repoRoot, '.git'), { throwIfNoEntry: false })) {
    const git = (args) => execute('git', ['--no-pager', ...args], {
      cwd: repoRoot, encoding: 'utf8', timeout: 10_000,
    });
    const { stdout } = await git(['ls-files', '--', ...patterns]);
    if (stdout.trim()) throw new Error('Private settings are tracked by Git; untrack them before writing');
    try {
      await Promise.all(names.flatMap((name) => [`.roster/${name}`, `.roster/${name}.tmp`])
        .map((file) => git(['check-ignore', '--quiet', '--', file])));
      ignored = true;
    } catch (error) {
      if (error.code !== 1) throw new Error('Could not verify private settings ignore rules', { cause: error });
    }
  } else {
    const lines = source.replace(/\r\n/g, '\n').split('\n');
    ignored = patterns.every((pattern) => lines.includes(pattern));
  }
  if (!ignored) {
    const separator = source && !source.endsWith('\n') ? '\n' : '';
    await fs.appendFile(ignoreFile, `${separator}${patterns.join('\n')}\n`, { encoding: 'utf8', mode: 0o600 });
  }
}

export async function writePrivateDocuments(documents, { repoRoot, fileSystem = fs } = {}) {
  if (!Array.isArray(documents) || !documents.length ||
      new Set(documents.map(({ name }) => name)).size !== documents.length) {
    throw new TypeError('Private writes need distinct documents');
  }
  const prepared = [];
  const committed = [];
  try {
    for (const document of documents) {
      if (typeof document.source !== 'string' || Buffer.byteLength(document.source, 'utf8') > 65_536) {
        throw new TypeError('Private settings must be UTF-8 text of at most 64 KiB');
      }
      const file = privatePath(repoRoot, document.name);
      const previous = await readPrivateFile(repoRoot, document.name);
      if (document.expectedSource !== undefined && previous !== document.expectedSource) {
        throw new Error('Private settings changed during setup; refusing to overwrite them');
      }
      const temporary = `${file}.${randomBytes(8).toString('hex')}.tmp`;
      prepared.push({ ...document, file, previous, temporary });
    }
    await fileSystem.mkdir(path.join(repoRoot, '.roster'), { recursive: true, mode: 0o700 });
    for (const document of prepared) {
      await ensureLocalPath(document.file, repoRoot);
      await fileSystem.writeFile(document.temporary, document.source, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
    }
    for (const document of prepared) {
      if (await readPrivateFile(repoRoot, document.name) !== document.previous) {
        throw new Error('Private settings changed before save; refusing to overwrite them');
      }
      await fileSystem.rename(document.temporary, document.file);
      committed.push(document);
    }
  } catch (error) {
    for (const document of committed.reverse()) {
      if (await readPrivateFile(repoRoot, document.name) !== document.source) {
        throw new Error('Private write failed and rollback was refused because a saved file changed', { cause: error });
      }
      if (document.previous === null) await fileSystem.unlink(document.file);
      else {
        await fileSystem.writeFile(document.temporary, document.previous, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
        await fileSystem.rename(document.temporary, document.file);
      }
    }
    throw error;
  } finally {
    for (const { temporary } of prepared) await fileSystem.rm(temporary, { force: true });
  }
}
