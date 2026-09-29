import { constants, promises as fs } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const rosterRoot = fileURLToPath(new URL('../../', import.meta.url));

export async function initializeRoster({
  cwd = process.cwd(),
  installationRoot = rosterRoot,
} = {}) {
  const files = [
    { source: path.join(installationRoot, 'roster.config.example.yml'),
      target: path.join(cwd, 'roster.config.example.yml') },
    { source: path.join(installationRoot, 'templates', 'init', 'POLICY-NOTE.md'),
      target: path.join(cwd, 'ROSTER-POLICY-NOTE.md') },
  ];
  const results = [];
  for (const { source, target } of files) {
    const original = await fs.lstat(source);
    if (!original.isFile() || original.isSymbolicLink()) {
      throw new Error('Roster init examples must be regular files');
    }
    let existing;
    try {
      existing = await fs.lstat(target);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
    if (existing) {
      if (!existing.isFile() || existing.isSymbolicLink()) {
        throw new Error(`Refusing to replace a non-regular init file: ${target}`);
      }
      results.push({ path: target, status: 'kept' });
      continue;
    }
    await fs.copyFile(source, target, constants.COPYFILE_EXCL);
    results.push({ path: target, status: 'created' });
  }
  return results;
}

export function formatInit(results) {
  if (!Array.isArray(results) || results.length !== 2 ||
      results.some((result) =>
        typeof result?.path !== 'string' || !['created', 'kept'].includes(result?.status))) {
    throw new TypeError('Expected config and policy-note init results');
  }
  return `${results.map(({ path, status }) =>
    `${status === 'created' ? 'Created' : 'Kept existing'}: ${path}`).join('\n')}\n` +
    'Policy: agent-policy.yml unchanged (human-owned)\n';
}
