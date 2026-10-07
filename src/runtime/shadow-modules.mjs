import { execFile } from 'node:child_process';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';
import { isForbiddenRead } from './tools.mjs';
import { isTestFile } from './test-substance.mjs';

const execute = promisify(execFile);
export const shadowPrefix = 'Shadow module:';

const codeFile = /\.[cm]?[jt]sx?$/;
const isProductCode = (file) => codeFile.test(file) && !isTestFile(file) && !file.startsWith('tests/') &&
  !file.startsWith('vendor/') && !isForbiddenRead(file);

// Verbs grouped by the role an export plays; the same role and subject in two modules suggests a shadow.
const roles = {
  read: ['read', 'load', 'get', 'fetch', 'list', 'find', 'lookup', 'query', 'all'],
  write: ['write', 'save', 'store', 'put', 'persist', 'append', 'insert', 'add', 'record'],
  validate: ['validate', 'check', 'verify', 'assert', 'ensure', 'require', 'is'],
  digest: ['hash', 'digest', 'checksum', 'sha', 'fingerprint'],
  parse: ['parse', 'decode', 'deserialize'],
  format: ['format', 'render', 'serialize', 'stringify', 'encode', 'to'],
  redact: ['redact', 'mask', 'scrub', 'sanitize'],
};
const roleOf = new Map(Object.entries(roles).flatMap(([role, verbs]) => verbs.map((verb) => [verb, role])));
const generic = new Set(['all', 'data', 'file', 'files', 'value', 'item', 'items', 'entry', 'entries', 'text', 'result',
  'options', 'config', 'default', 'new', 'from', 'by', 'of', 'for', 'and', 'or', 'with', 'js', 'mjs', 'index', 'util', 'utils',
  'lib', 'helper', 'helpers', 'store', 'state']);

export function words(name) {
  return name.replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/([A-Z]+)([A-Z][a-z])/g, '$1 $2')
    .split(/[^A-Za-z0-9]+/).filter(Boolean).map((word) => word.toLowerCase());
}

const singular = (word) => word.length > 3 && word.endsWith('ies') ? `${word.slice(0, -3)}y`
  : word.length > 3 && word.endsWith('s') && !word.endsWith('ss') ? word.slice(0, -1) : word;

export function profile(name) {
  const parts = words(name);
  const role = parts.map((word) => roleOf.get(word)).find(Boolean) ?? null;
  const nouns = new Set(parts.filter((word) => !roleOf.has(word) || word === 'record').map(singular)
    .filter((word) => !generic.has(word) && word.length > 2));
  return { role, nouns };
}

export function exportNames(text) {
  const names = new Set();
  for (const match of (text ?? '').matchAll(/^export\s+(?:default\s+)?(?:async\s+)?(?:function\*?|class|const|let|var)\s+([A-Za-z_$][\w$]*)/gm)) {
    names.add(match[1]);
  }
  // A re-export (`export { x } from './y.mjs'`) reuses a module, so it is not a new definition.
  for (const match of (text ?? '').matchAll(/^export\s*\{([^}]*)\}(?!\s*from\b)/gm)) {
    for (const part of match[1].split(',')) {
      const name = part.trim().split(/\s+as\s+/).at(-1)?.trim();
      if (name && /^[A-Za-z_$][\w$]*$/.test(name) && name !== 'default') names.add(name);
    }
  }
  return [...names];
}

// Public entry points are called by users, not by other modules.
const isEntryPoint = (file) => file === 'src/cli.mjs' || file.startsWith('bin/');
const occurrences = (text, name) =>
  (text ?? '').match(new RegExp(`(?<![\\w$])${name.replaceAll('$', '\\$')}(?![\\w$])`, 'g'))?.length ?? 0;
const mentions = (text, name) => occurrences(text, name) > 0;

const stemWords = (file) => words(path.posix.basename(file).replace(codeFile, '')).map(singular)
  .filter((word) => !generic.has(word) && word.length > 2);

// Deterministic, LLM-free: new exports and files in the diff compared with existing modules by name, stem, and role.
export async function checkShadowModules({ worktree, files, priorWaveFiles = [], taskText = '', runCommand = execute }) {
  const changed = [...new Set(files.map((file) => file.replaceAll('\\', '/')))].filter(isProductCode);
  if (!changed.length) return { status: 'none', findings: [] };
  const git = async (args) => (await runCommand('git', args, { cwd: worktree, encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024, timeout: 30_000, windowsHide: true })).stdout;
  let tracked;
  try {
    tracked = new Set((await git(['ls-files', '-z'])).split('\0').filter(Boolean));
  } catch {
    return { status: 'unavailable', findings: [] };
  }
  const read = (file) => fs.readFile(path.join(worktree, file), 'utf8').catch(() => null);
  const base = (file) => tracked.has(file) ? git(['show', `HEAD:${file}`]).catch(() => null) : null;
  const added = [];
  for (const file of changed) {
    const [now, before] = await Promise.all([read(file), base(file)]);
    if (now === null) continue;
    const previous = new Set(exportNames(before));
    for (const name of exportNames(now)) if (!previous.has(name)) added.push({ file, name, newFile: before === null });
  }
  if (!added.length) return { status: 'checked', findings: [] };
  const earlier = new Set(priorWaveFiles.map((file) => file.replaceAll('\\', '/')));
  const modules = [];
  const sources = new Map();
  for (const file of new Set([...tracked, ...changed])) {
    if (isProductCode(file)) sources.set(file, await read(file));
  }
  for (const file of [...tracked].filter(isProductCode)) {
    const text = changed.includes(file) ? await base(file) : sources.get(file);
    const names = exportNames(text);
    if (!names.length) continue;
    modules.push({ file, names, stem: new Set(stemWords(file)), profiles: names.map((name) => ({ name, ...profile(name) })) });
  }
  const findings = [];
  for (const { file, name, newFile } of added) {
    const same = modules.find((module) => module.file !== file && module.names.includes(name));
    const label = (module) => `${module.file}${earlier.has(module.file) ? ' (earlier wave)' : ''}`;
    if (same) {
      findings.push({ file, name, existing: same.file, reason: `${shadowPrefix} ${file} adds ${name}, which ${label(same)} ` +
        `already exports; import or extend ${name} in ${same.file} instead.` });
      continue;
    }
    if (!newFile) continue;
    const { role, nouns } = profile(name);
    if (!role || !nouns.size) continue;
    // The subject must be the module's own (its file stem) or that of one export with the same role.
    const covers = (set) => [...nouns].every((noun) => set.has(noun));
    let peer;
    const match = modules.find((module) => {
      if (module.file === file) return false;
      const sameRole = module.profiles.filter((entry) => entry.role === role);
      peer = sameRole.find((entry) => entry.nouns.size === nouns.size && covers(entry.nouns)) ??
        (covers(module.stem) ? sameRole[0] : undefined);
      return Boolean(peer);
    });
    if (match) {
      findings.push({ file, name, existing: match.file, reason: `${shadowPrefix} new file ${file} adds ${name}, which ` +
        `duplicates the ${role} role of ${peer.name} in ${label(match)}; extend ${match.file} instead of a parallel module.` });
    }
  }
  const flagged = new Set(findings.map(({ file, name }) => `${file}\0${name}`));
  for (const { file, name } of added) {
    if (flagged.has(`${file}\0${name}`) || isEntryPoint(file) || mentions(taskText, name)) continue;
    const used = [...sources].some(([other, text]) => text !== null &&
      (other === file ? occurrences(text, name) > 1 : occurrences(text, name) > 0));
    if (used) continue;
    findings.push({ file, name, unused: true, reason: `${shadowPrefix} ${file} exports ${name}, but no product module ` +
      'uses it (only tests, or nothing); wire it into the production caller the task names, make it module-private, ' +
      'or delete it. If a later wave owns the caller, say so in RESULT.md.' });
  }
  return { status: findings.length ? 'flagged' : 'checked', findings, exports: added.length };
}

export function shadowSection(shadow) {
  return `Status: ${shadow.status}\n\n` + (shadow.findings.length
    ? shadow.findings.map(({ reason }) => `- ${reason.slice(shadowPrefix.length + 1)}`).join('\n')
    : '- No new export duplicates an existing module or lacks a product caller.') + '\n';
}
