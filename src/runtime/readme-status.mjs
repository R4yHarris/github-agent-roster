import { parseTaskDocument } from '../planner/task.mjs';

function validateTask(task) {
  const document = parseTaskDocument(task);
  if (document.files_allowed.length !== 1 || document.files_allowed[0] !== 'README.md' ||
      !/(?:status\s+section|section.{0,30}status|##\s*status)/i.test(`${document.title}\n${document.ask}`)) {
    throw new Error('Deterministic Status fallback requires an explicit Status task scoped only to README.md');
  }
}

async function readStatus({ task, tools }) {
  validateTask(task);
  if ((await tools.read_file({ path: 'TASK.md' })).replace(/\r\n/g, '\n') !== task.replace(/\r\n/g, '\n')) {
    throw new Error('TASK.md changed before deterministic fallback');
  }
  const text = await tools.read_file({ path: 'README.md' });
  const newline = text.includes('\r\n') ? '\r\n' : '\n';
  const lines = text.split(/\r?\n/);
  let fenced = false;
  let existing = -1;
  let insert = -1;
  for (const [index, line] of lines.entries()) {
    if (/^ {0,3}(?:`{3,}|~{3,})/.test(line)) fenced = !fenced;
    if (fenced) continue;
    if (/^## Status[ \t]*$/.test(line)) existing = index;
    if (insert < 0 && /^##\s+/.test(line)) insert = index;
  }
  const end = existing < 0 ? -1 : lines.findIndex((line, index) => index > existing && /^#{1,2}\s+/.test(line));
  const body = existing < 0 ? [] :
    lines.slice(existing + 1, end < 0 ? lines.length : end).filter((line) => line.trim());
  return { text, newline, lines, existing, insert, body };
}

export async function hasRequiredReadmeStatus(options) {
  const { existing, body } = await readStatus(options);
  return existing >= 0 && body.length === 1;
}

export async function applyReadmeStatus({ task, tools }) {
  const { text, newline, lines, existing, insert, body } = await readStatus({ task, tools });
  if (existing >= 0) {
    if (body.length !== 1) throw new Error('Existing Status section is not one line; refusing an automatic rewrite');
    return 'README.md already has a one-line Status section; no deterministic edit was needed.';
  }
  const offset = insert < 0 ? text.length : lines.slice(0, insert).reduce((total, line) => total + line.length + newline.length, 0);
  const before = text.slice(0, offset);
  const after = text.slice(offset);
  const separator = before.endsWith(`${newline}${newline}`) ? '' : before.endsWith(newline) ? newline : `${newline}${newline}`;
  const ending = after ? `${newline}${newline}` : text.endsWith('\n') ? newline : '';
  await tools.write_file({ path: 'README.md',
    content: `${before}${separator}## Status${newline}Experimental - APIs may change.${ending}${after}` });
  return 'Added ## Status and exactly one body line to README.md using the deterministic fallback.';
}
