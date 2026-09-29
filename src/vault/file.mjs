import { execFile } from 'node:child_process';
import { createCipheriv, createDecipheriv, randomBytes } from 'node:crypto';
import { constants, promises as fs } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { promisify } from 'node:util';

const execute = promisify(execFile);
class VaultError extends Error {}

export function validateSecretName(name) {
  if (typeof name !== 'string' || !/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name)) {
    throw new TypeError('Secret names must be 1-64 ASCII letters, digits or underscores, starting with a letter or underscore.');
  }
}

async function statIfPresent(path) {
  try {
    return await fs.lstat(path);
  } catch (error) {
    if (error.code === 'ENOENT') return undefined;
    throw error;
  }
}

async function outsideGit(directory) {
  let ancestor = directory;
  while (!(await statIfPresent(ancestor))) {
    const parent = dirname(ancestor);
    if (parent === ancestor) throw new VaultError('The vault location has no accessible filesystem root.');
    ancestor = parent;
  }
  ancestor = await fs.realpath(ancestor);
  while (true) {
    if (await statIfPresent(join(ancestor, '.git'))) {
      throw new VaultError('The file vault must be outside a Git worktree.');
    }
    const parent = dirname(ancestor);
    if (parent === ancestor) break;
    ancestor = parent;
  }
}

async function secureDirectory(directory) {
  if (process.platform !== 'win32') {
    await fs.chmod(directory, 0o700);
    return;
  }
  // Node's chmod does not enforce owner-only access on Windows.
  const script = [
    "$ErrorActionPreference = 'Stop'",
    '$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User',
    '$acl = [System.Security.AccessControl.DirectorySecurity]::new()',
    '$acl.SetOwner($sid)',
    '$acl.SetAccessRuleProtection($true, $false)',
    "$rule = [System.Security.AccessControl.FileSystemAccessRule]::new($sid, 'FullControl', 'ContainerInherit, ObjectInherit', 'None', 'Allow')",
    '$acl.AddAccessRule($rule)',
    '[System.IO.Directory]::SetAccessControl($env:ROSTER_VAULT_DIRECTORY, $acl)',
  ].join('; ');
  await execute('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], {
    env: { ...process.env, ROSTER_VAULT_DIRECTORY: directory },
    windowsHide: true,
    timeout: 10_000,
  });
}

export function createFileVault({ directory = join(homedir(), '.roster', 'vault') } = {}) {
  if (typeof directory !== 'string' || !directory.trim()) {
    throw new TypeError('The vault directory must be a non-empty path.');
  }
  directory = resolve(directory);
  const keyPath = join(directory, '.key');
  const entryPath = (name) => join(directory, `${Buffer.from(name).toString('hex')}.json`);

  async function prepare(create) {
    await outsideGit(directory);
    let stat = await statIfPresent(directory);
    if (!stat && !create) return false;
    if (!stat) {
      await fs.mkdir(directory, { recursive: true, mode: 0o700 });
      stat = await fs.lstat(directory);
    }
    if (!stat.isDirectory() || stat.isSymbolicLink()) {
      throw new VaultError('The vault location must be a directory, not a symbolic link.');
    }
    await secureDirectory(directory);
    return true;
  }

  async function readFile(path) {
    const stat = await statIfPresent(path);
    if (!stat) return undefined;
    if (!stat.isFile() || stat.isSymbolicLink()) {
      throw new VaultError('Vault files must be regular files, not symbolic links.');
    }
    const file = await fs.open(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
    try {
      if (!(await file.stat()).isFile()) throw new VaultError('Vault files must be regular files.');
      if (process.platform === 'win32') {
        await execute('icacls.exe', [path, '/reset', '/Q'], { windowsHide: true, timeout: 10_000 });
      } else {
        await file.chmod(0o600);
      }
      return await file.readFile();
    } finally {
      await file.close();
    }
  }

  async function names() {
    const entries = await fs.readdir(directory, { withFileTypes: true });
    const result = [];
    for (const entry of entries) {
      if (!entry.name.endsWith('.json')) continue;
      const match = /^([a-f0-9]{2,128})\.json$/.exec(entry.name);
      const name = match && Buffer.from(match[1], 'hex').toString('utf8');
      if (!entry.isFile() || !name || !/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name) ||
          Buffer.from(name).toString('hex') !== match[1]) {
        throw new VaultError('The vault contains an invalid entry.');
      }
      result.push(name);
    }
    return result.sort();
  }

  async function writeAtomic(path, content, exclusive = false) {
    const existing = await statIfPresent(path);
    if (existing && (!existing.isFile() || existing.isSymbolicLink())) {
      throw new VaultError('Vault files must be regular files, not symbolic links.');
    }
    const temporary = join(directory, `.tmp-${randomBytes(16).toString('hex')}`);
    const file = await fs.open(temporary, 'wx', 0o600);
    try {
      try {
        await file.writeFile(content);
        await file.sync();
      } finally {
        await file.close();
      }
      // Linking a complete key publishes it exclusively, including across processes.
      if (exclusive) await fs.link(temporary, path);
      else await fs.rename(temporary, path);
    } finally {
      await fs.rm(temporary, { force: true });
    }
  }

  async function readKey(create) {
    let key = await readFile(keyPath);
    if (!key && create) {
      if ((await names()).length) {
        key = await readFile(keyPath);
        if (!key) throw new VaultError('The vault key is missing; restore it before writing.');
      }
    }
    if (!key && create) {
      const generated = randomBytes(32);
      try {
        await writeAtomic(keyPath, generated, true);
      } catch (error) {
        if (error.code !== 'EEXIST') throw error;
      } finally {
        generated.fill(0);
      }
      key = await readFile(keyPath);
    }
    if (!key) throw new VaultError('The vault key is missing.');
    if (key.length !== 32) {
      key.fill(0);
      throw new VaultError('The vault key must contain 32 bytes.');
    }
    return key;
  }

  return {
    async set(name, value) {
      validateSecretName(name);
      if (typeof value !== 'string' || value.length === 0) {
        throw new TypeError('A secret must be a non-empty string.');
      }
      try {
        await prepare(true);
        const key = await readKey(true);
        try {
          const iv = randomBytes(12);
          const cipher = createCipheriv('aes-256-gcm', key, iv);
          cipher.setAAD(Buffer.from(name));
          const ciphertext = Buffer.concat([cipher.update(value, 'utf8'), cipher.final()]);
          await writeAtomic(entryPath(name), JSON.stringify({
            version: 1,
            iv: iv.toString('base64'),
            tag: cipher.getAuthTag().toString('base64'),
            ciphertext: ciphertext.toString('base64'),
          }));
        } finally {
          key.fill(0);
        }
      } catch (error) {
        if (error instanceof VaultError) throw error;
        throw new VaultError('Unable to write the file vault. Check its location and permissions.');
      }
    },

    async get(name) {
      validateSecretName(name);
      try {
        if (!(await prepare(false))) return undefined;
        const stored = await readFile(entryPath(name));
        if (!stored) return undefined;
        const key = await readKey(false);
        try {
          const entry = JSON.parse(stored.toString('utf8'));
          if (entry?.version !== 1 || typeof entry.iv !== 'string' ||
              typeof entry.tag !== 'string' || typeof entry.ciphertext !== 'string') {
            throw new VaultError('The vault entry format is invalid.');
          }
          const iv = Buffer.from(entry.iv, 'base64');
          const tag = Buffer.from(entry.tag, 'base64');
          if (iv.length !== 12 || tag.length !== 16) {
            throw new VaultError('The vault entry format is invalid.');
          }
          const decipher = createDecipheriv('aes-256-gcm', key, iv);
          decipher.setAAD(Buffer.from(name));
          decipher.setAuthTag(tag);
          return Buffer.concat([
            decipher.update(Buffer.from(entry.ciphertext, 'base64')),
            decipher.final(),
          ]).toString('utf8');
        } catch {
          throw new VaultError('The vault entry is invalid or could not be authenticated.');
        } finally {
          key.fill(0);
        }
      } catch (error) {
        if (error instanceof VaultError) throw error;
        throw new VaultError('Unable to read the file vault. Check its location and permissions.');
      }
    },

    async list() {
      try {
        return await prepare(false) ? await names() : [];
      } catch (error) {
        if (error instanceof VaultError) throw error;
        throw new VaultError('Unable to list the file vault. Check its location and permissions.');
      }
    },
  };
}
