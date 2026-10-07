import { promises as fs } from 'node:fs';
import path from 'node:path';
import { isAllowedFile, isForbiddenRead } from '../runtime/tools.mjs';
import { relatedExports } from './related-exports.mjs';
import { oneLine, taskSections } from './task.mjs';

const codeFile = /\.(?:[cm]?js|[cm]?ts|jsx|tsx)$/;
const exportPattern = /^export\s+(?:default\s+)?(?:async\s+)?(?:function\*?|const|let|class)\s+([A-Za-z_$][\w$]*)/gm;
const identifier = /^[A-Za-z_$][\w$]*$/;
const designLabels = { 'extend': 'extend', 'new exports': 'new_exports', 'outline': 'outline', 'edge cases': 'edge_cases',
  'out of scope': 'out_of_scope' };
const byteLimit = 256 * 1024;

const codeLike = (name) => identifier.test(name) && !/^[A-Z0-9_]+$/.test(name) &&
  (/[a-z0-9][A-Z]/.test(name) || /^[a-z]+(?:_[a-z0-9]+)+$/.test(name));

// Every identifier token in tracked code, plus each file's exports: what a plan may cite as existing.
export async function symbolIndex(worktree, repositoryFiles = []) {
  const symbols = new Set();
  const exportsByFile = new Map();
  for (const file of repositoryFiles.filter((entry) => codeFile.test(entry) && !entry.startsWith('vendor/') &&
    !isForbiddenRead(entry)).slice(0, 1500)) {
    let text;
    try {
      const target = path.join(worktree, file);
      if ((await fs.lstat(target)).size > byteLimit) continue;
      text = await fs.readFile(target, 'utf8');
    } catch { continue; }
    for (const token of text.match(/[A-Za-z_$][\w$]*/g) ?? []) symbols.add(token);
    const names = new Set([...text.matchAll(exportPattern)].map((match) => match[1]));
    for (const match of text.matchAll(/\bexports\.([A-Za-z_$][\w$]*)\s*=/g)) names.add(match[1]);
    for (const match of text.matchAll(/^export\s*\{([^}]*)\}/gm)) {
      for (const part of match[1].split(',')) {
        const name = part.trim().split(/\s+as\s+/).pop();
        if (identifier.test(name ?? '')) names.add(name);
      }
    }
    exportsByFile.set(file, names);
  }
  return { symbols, exportsByFile, files: new Set(repositoryFiles) };
}

function definition(text, name) {
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  const start = lines.findIndex((line) => new RegExp(
    `^export\\s+(?:default\\s+)?(?:async\\s+)?(?:function\\*?|const|let|class)\\s+${name.replace(/\$/g, '\\$')}\\b`).test(line));
  if (start < 0) return null;
  let depth = 0;
  let opened = false;
  const body = [];
  for (let index = start; index < lines.length && body.length < 40; index += 1) {
    body.push(lines[index]);
    for (const char of lines[index].replace(/(['"`])(?:\\.|(?!\1).)*\1/g, '')) {
      if (char === '{') { depth += 1; opened = true; }
      else if (char === '}') depth -= 1;
    }
    if (opened ? depth <= 0 : /;\s*$/.test(lines[index])) break;
  }
  return body.length > 14 ? [...body.slice(0, 12), '  // ...', body.at(-1)].join('\n') : body.join('\n');
}

// Spec 5.3: real definition bodies for the Ask's nouns, so checks cite existing exports, not concepts.
export async function groundingDefinitions(worktree, repositoryFiles, askText, { charLimit = 4000 } = {}) {
  const related = await relatedExports(worktree, repositoryFiles, askText, { limit: 6, exportsPerFile: 4 });
  const blocks = [];
  let used = 0;
  for (const { file, exports } of related) {
    let text;
    try { text = await fs.readFile(path.join(worktree, file), 'utf8'); } catch { continue; }
    for (const name of exports) {
      const body = definition(text, name);
      if (!body) continue;
      const block = `\`${file}\` \`${name}\`\n${body}`;
      if (used + block.length > charLimit) return { related, text: blocks.join('\n\n') };
      blocks.push(block);
      used += block.length + 2;
    }
  }
  return { related, text: blocks.join('\n\n') };
}

function citations(text) {
  const names = new Set();
  const paths = new Set();
  for (const [, raw] of String(text ?? '').matchAll(/`([^`\n]+)`/g)) {
    const span = raw.trim();
    if (/^[\w.@-]+(?:\/[\w.@-]+)+\.\w+$/.test(span)) { paths.add(span); continue; }
    const call = span.match(/^([A-Za-z_$][\w$.]*)\s*\(/)?.[1] ?? span;
    if (!/^[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*$/.test(call)) continue;
    for (const part of call.split('.')) if (codeLike(part)) names.add(part);
  }
  return { names: [...names], paths: [...paths] };
}

function list(value, label, limit, entry = (item) => oneLine(item, label)) {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > limit) throw new TypeError(`design.${label} must be a list of at most ${limit} entries`);
  return value.map(entry);
}

export function normalizeDesign(design) {
  if (design === undefined || design === null) return null;
  if (typeof design !== 'object' || Array.isArray(design) ||
      Object.keys(design).some((key) => !Object.values(designLabels).includes(key))) {
    throw new TypeError(`design must be an object with ${Object.values(designLabels).join(', ')}`);
  }
  const name = (value, label) => {
    const text = oneLine(value, label).replace(/^`|`$/g, '').replace(/\(.*$/, '');
    if (!identifier.test(text)) throw new TypeError(`${label} must be an identifier, got ${text}`);
    return text;
  };
  const file = (value, label) => oneLine(value, label).replace(/^`|`$/g, '');
  return {
    extend: list(design.extend, 'extend', 8, (item) => {
      if (!item || typeof item !== 'object') throw new TypeError('design.extend entries need file and exports');
      return { file: file(item.file, 'design.extend file'),
        exports: list(item.exports, 'extend exports', 12, (value) => name(value, 'design.extend export')) };
    }),
    new_exports: list(design.new_exports, 'new_exports', 12, (item) => {
      if (!item || typeof item !== 'object') throw new TypeError('design.new_exports entries need file and name');
      return { file: file(item.file, 'design.new_exports file'), name: name(item.name, 'design.new_exports name') };
    }),
    outline: list(design.outline, 'outline', 10),
    edge_cases: list(design.edge_cases, 'edge_cases', 8),
    out_of_scope: list(design.out_of_scope, 'out_of_scope', 6),
  };
}

// Rejects plans whose cited code names or paths do not exist and are not declared as new in an allowed file.
export function groundingErrors({ checks = [], design = null, filesAllowed = [], askText = '', index }) {
  if (!index) return [];
  const errors = [];
  const declared = new Set((design?.new_exports ?? []).map(({ name }) => name));
  for (const item of design?.extend ?? []) {
    if (!index.files.has(item.file)) {
      errors.push(`design.extend names \`${item.file}\`, which is not a tracked file`);
      continue;
    }
    const exported = index.exportsByFile.get(item.file);
    for (const name of item.exports) {
      if (!(exported?.size ? exported.has(name) : index.symbols.has(name))) {
        errors.push(`design.extend cites \`${name}\`, which \`${item.file}\` does not export`);
      }
    }
  }
  for (const { file, name } of design?.new_exports ?? []) {
    if (!isAllowedFile(file, filesAllowed)) errors.push(`design.new_exports puts \`${name}\` in \`${file}\`, outside files_allowed`);
    const owner = [...index.exportsByFile].find(([other, names]) => names.has(name) && other !== file &&
      !isAllowedFile(other, filesAllowed))?.[0];
    if (owner) errors.push(`design.new_exports \`${name}\` already exists in \`${owner}\`; extend that module instead`);
  }
  const text = [...checks, ...(design?.outline ?? []), ...(design?.edge_cases ?? [])].join('\n');
  const { names, paths } = citations(text);
  for (const name of names) {
    if (index.symbols.has(name) || declared.has(name) || askText.includes(name)) continue;
    errors.push(`\`${name}\` does not exist in the repository; cite a real export or declare it in design.new_exports`);
  }
  for (const file of paths) {
    if (index.files.has(file) || isAllowedFile(file, filesAllowed) || askText.includes(file)) continue;
    errors.push(`\`${file}\` is neither a tracked file nor in files_allowed`);
  }
  return errors;
}

export function deriveDesign({ filesAllowed, index, related = [] }) {
  const extend = [];
  for (const file of filesAllowed.filter((entry) => index?.files.has(entry) && codeFile.test(entry))) {
    extend.push({ file, exports: [...(index.exportsByFile.get(file) ?? [])].slice(0, 8) });
  }
  for (const { file, exports } of related) {
    if (extend.length >= 6) break;
    if (!extend.some((entry) => entry.file === file)) extend.push({ file, exports: exports.slice(0, 6) });
  }
  return { extend, new_exports: [], outline: [
    'Read every Extend module first; reuse its exports instead of adding a parallel module.',
    'Write or extend a test for each acceptance check, then change only Files allowed.',
  ], edge_cases: [], out_of_scope: ['Files outside Files allowed; new runtime dependencies'], derived: true };
}

export function renderDesign(design) {
  const rows = ['## Design', ''];
  if (design.derived) rows.push('Source: derived by the harness from the repository; the planner gave none.', '');
  if (design.rejected) rows.push(`The planner's design was rejected: ${design.rejected}.`, '');
  if (design.extend.length) rows.push('Extend:', ...design.extend.map(({ file, exports }) =>
    `- \`${file}\`${exports.length ? `: ${exports.map((name) => `\`${name}\``).join(', ')}` : ''}`));
  if (design.new_exports.length) rows.push('New exports:', ...design.new_exports.map(({ file, name }) => `- \`${name}\` in \`${file}\``));
  if (design.outline.length) rows.push('Outline:', ...design.outline.map((step, index) => `${index + 1}. ${step}`));
  if (design.edge_cases.length) rows.push('Edge cases:', ...design.edge_cases.map((item) => `- ${item}`));
  if (design.out_of_scope.length) rows.push('Out of scope:', ...design.out_of_scope.map((item) => `- ${item}`));
  return `${rows.join('\n')}\n`;
}

export function parseDesign(task) {
  const section = taskSections(task).sections.find(({ name }) => name === 'design');
  if (!section) return null;
  const design = { extend: [], new_exports: [], outline: [], edge_cases: [], out_of_scope: [] };
  let key = null;
  for (const line of section.content.split('\n').map((entry) => entry.trim()).filter(Boolean)) {
    const label = designLabels[line.replace(/:$/, '').toLowerCase()];
    if (label && line.endsWith(':')) { key = label; continue; }
    const item = line.replace(/^(?:[-*+]|\d+[.)])\s+/, '');
    if (key === 'extend') {
      const match = item.match(/^`([^`]+)`(?::\s*(.*))?$/);
      if (match) design.extend.push({ file: match[1], exports: [...(match[2] ?? '').matchAll(/`([^`]+)`/g)].map((entry) => entry[1]) });
    } else if (key === 'new_exports') {
      const match = item.match(/^`([^`]+)`\s+in\s+`([^`]+)`/);
      if (match) design.new_exports.push({ name: match[1], file: match[2] });
    } else if (key) design[key].push(item);
  }
  return design;
}

export function withDesign(task, design) {
  if (taskSections(task).sections.some(({ name }) => name === 'design')) return task;
  const block = renderDesign(design);
  const heading = /^## (?:Files allowed|Allowed Files)\s*$/im;
  return heading.test(task) ? task.replace(heading, `${block}\n$&`) : `${task.trimEnd()}\n\n${block}`;
}

export const isCodeSlice = (files) => files.some((file) => codeFile.test(file));

// Coder-side gate: a TASK.md Design must still hold against this worktree before the first product write.
export async function designGateErrors({ worktree, task, filesAllowed, repositoryFiles }) {
  const design = parseDesign(task);
  if (!design || !repositoryFiles?.length) return [];
  const askText = taskSections(task).sections.find(({ name }) => name === 'ask')?.content ?? '';
  return groundingErrors({ design, filesAllowed, askText, index: await symbolIndex(worktree, repositoryFiles) });
}
