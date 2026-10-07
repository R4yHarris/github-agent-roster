import { promises as fs } from 'node:fs';
import path from 'node:path';
import { isForbiddenRead } from './tools.mjs';

const codeFile = /\.[cm]?js$/;
const byteLimit = 256 * 1024;
// camelCase properties read off a record-like object, for example `record.repoIdentity`.
const fieldAccess = /(?:\b[a-z][A-Za-z0-9]*|[)\]])\??\.([a-z][a-z0-9]*[A-Z][A-Za-z0-9]*)\b(?!\s*\()/g;
const camelCase = /\b([a-z][a-z0-9]*[A-Z][A-Za-z0-9]*)\b/g;
// Lines that state what a value must be: thrown messages, type checks, and pattern tests.
const contractLine = /\bmust\b|\bthrow\b|TypeError|RangeError|\.test\(|\btypeof\b|\binstanceof\b|Number\.is|Array\.isArray/;

async function sourceFiles(root, directory = 'src', found = []) {
  let entries;
  try { entries = await fs.readdir(path.join(root, directory), { withFileTypes: true }); } catch { return found; }
  for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
    const file = `${directory}/${entry.name}`;
    if (isForbiddenRead(file)) continue;
    if (entry.isDirectory()) await sourceFiles(root, file, found);
    else if (entry.isFile() && codeFile.test(entry.name)) found.push(file);
  }
  return found;
}

async function readText(root, file) {
  try {
    const target = path.join(root, file);
    if ((await fs.lstat(target)).size > byteLimit) return null;
    return await fs.readFile(target, 'utf8');
  } catch {
    return null;
  }
}

// Spec 5.3: validators and shapes of the record fields a slice touches, so a coder cannot treat a hash as a path.
export async function fieldContracts(worktree, { files = [], taskText = '', limit = 1500, maxFields = 6, perField = 3 } = {}) {
  const root = path.resolve(worktree);
  const sources = await sourceFiles(root);
  const texts = new Map();
  for (const file of sources) texts.set(file, await readText(root, file));
  const accessed = new Set();
  for (const file of files.map((entry) => entry.replaceAll('\\', '/'))) {
    const text = texts.get(file) ?? (codeFile.test(file) && !isForbiddenRead(file) ? await readText(root, file) : null);
    for (const [, field] of (text ?? '').matchAll(fieldAccess)) accessed.add(field);
  }
  const named = new Set([...String(taskText).matchAll(camelCase)].map(([, field]) => field));
  const candidates = [...new Set([...named, ...accessed])];
  const blocks = [];
  for (const field of candidates) {
    const word = new RegExp(`(?<![\\w$])${field}(?![\\w$])`);
    const lines = [];
    for (const [file, text] of texts) {
      if (text === null || !word.test(text)) continue;
      for (const [index, line] of text.replace(/\r\n/g, '\n').split('\n').entries()) {
        if (!word.test(line) || !contractLine.test(line)) continue;
        const trimmed = line.trim();
        lines.push({ must: /\bmust\b/.test(line), entry: `- ${file}:${index + 1}: ${trimmed.length > 160 ? `${trimmed.slice(0, 157)}...` : trimmed}` });
      }
    }
    if (!lines.some(({ must }) => must)) continue;
    lines.sort((a, b) => Number(b.must) - Number(a.must));
    blocks.push({ named: named.has(field), body: `\`${field}\`\n${lines.slice(0, perField).map(({ entry }) => entry).join('\n')}` });
  }
  blocks.sort((a, b) => Number(b.named) - Number(a.named));
  const kept = [];
  let used = 0;
  for (const { body } of blocks.slice(0, maxFields)) {
    if (used + body.length > limit) {
      if (kept.length === 0) kept.push(`${body.slice(0, Math.max(0, limit - 3))}...`);
      break;
    }
    kept.push(body);
    used += body.length + 2;
  }
  return kept.join('\n\n');
}
