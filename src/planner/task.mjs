import { splitTaskFrontmatter } from '../runtime/skills.mjs';
import { isForbiddenWrite, planArtifactFiles, plannerArtifactFiles } from '../runtime/tools.mjs';

export function oneLine(value, label) {
  const collapsed = typeof value === 'string' ? value.replace(/[\x00-\x1f\x7f]+/g, ' ').replace(/\s+/g, ' ').trim() : '';
  if (!collapsed) throw new TypeError(`${label} must be one nonempty line (at most 240 characters)`);
  return collapsed.slice(0, 240);
}

export function allowedFile(value) {
  const file = oneLine(value, 'Files allowed entry');
  if (!/^(?:\*\*\/\*|[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)*(?:\/\*\*)?)$/.test(file) ||
      file.split('/').some((part) => part === '.' || part === '..') || isForbiddenWrite(file)) {
    throw new TypeError('Files allowed entries must stay inside the worktree and exclude protected files');
  }
  return file;
}

export function checkedList(items, label, check, limit = 8) {
  if (!Array.isArray(items) || items.length < 1 || items.length > limit) {
    throw new TypeError(`${label} must contain 1-${limit} entries` +
      (Array.isArray(items) && items.length > limit ? ` (got ${items.length}; merge related entries)` : ''));
  }
  return items.map(check);
}

function sectionName(heading) {
  const name = heading.toLowerCase().replace(/[_-]+/g, ' ').replace(/\s+/g, ' ').replace(/:$/, '').trim()
    .replace(/^(ask|original ask)\s*\([^)]*\)$/, '$1');
  if (name === 'original ask') return 'ask';
  if (name === 'allowed files') return 'files allowed';
  return name;
}

export function taskSections(task) {
  if (typeof task !== 'string' || Buffer.byteLength(task, 'utf8') > 65_536 ||
      /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(task)) {
    throw new TypeError('TASK.md must be UTF-8 text of at most 64 KiB without control characters');
  }
  const { frontmatter, body: text } = splitTaskFrontmatter(task);
  if (text.includes('\r')) throw new TypeError('TASK.md contains a lone carriage return');
  const titleHeading = /^# +(.+?)[ \t]*$/m.exec(text);
  if (!titleHeading || text.slice(0, titleHeading.index).trim()) throw new TypeError('TASK.md needs a title heading');
  const heading = titleHeading[1].replace(/^(?:task|title):[ \t]*/i, '');
  // A template-copied "# Task title" heading takes the real title from the first plain line under it.
  const promoted = /^(?:task[ \t]*)?title$|^task$/i.test(heading)
    ? text.slice(titleHeading.index + titleHeading[0].length).split('\n').map((line) => line.trim()).find(Boolean)
    : undefined;
  const title = oneLine((promoted && !/^(?:[#>|-]|\w+:)/.test(promoted) ? promoted : heading) || 'Task', 'Task title');
  const starts = [];
  const known = new Set();
  let offset = 0;
  let fence;
  for (const line of text.split('\n')) {
    const fenced = /^ {0,3}(`{3,}|~{3,})/.exec(line)?.[1];
    if (fenced) {
      if (!fence) fence = fenced[0];
      else if (fenced[0] === fence) fence = undefined;
    } else if (!fence) {
      const heading = /^(#{2,6}) +(.+?)[ \t]*$/.exec(line);
      if (heading) {
        const name = sectionName(heading[2]);
        if (['ask', 'acceptance checks', 'files allowed', 'metadata', 'planning failure'].includes(name) &&
            known.has(name)) throw new TypeError(`TASK.md has duplicate ${name} sections`);
        known.add(name);
        starts.push({ name, start: offset, contentStart: offset + line.length + 1 });
        if (name === 'ask' && !/^original[ _-]+ask\b/i.test(heading[2]) &&
            known.has('acceptance checks') && known.has('files allowed')) break;
      }
    }
    offset += line.length + 1;
  }
  const sections = starts.map((section, index) => {
    const end = starts[index + 1]?.start ?? text.length;
    return { ...section, content: text.slice(section.contentStart, end).trim(),
      source: text.slice(section.start, end) };
  });
  const bodyStart = starts[0]?.start ?? text.length;
  return { frontmatter, title, header: text.slice(0, bodyStart), body: text.slice(bodyStart), sections };
}

function sectionList(section, label, { continuations = false } = {}) {
  if (!section?.content) throw new TypeError(`TASK.md must contain ${label}`);
  const items = [];
  for (const line of section.content.split('\n').filter((line) => line.trim())) {
    const value = line.trim().replace(/^(?:[-*+]|\d+[.)])[ \t]+/, '');
    if (continuations && /^\s+\S/.test(line) && items.length) items[items.length - 1] += ` ${value}`;
    else items.push(value.replace(/^`([^`]+)`$/, '$1'));
  }
  return items;
}

export function applicationFiles(items) {
  return checkedList(items.filter((file) => ![...plannerArtifactFiles, ...planArtifactFiles].some((name) =>
    typeof file === 'string' && file.toLowerCase() === name.toLowerCase())),
  'Files allowed', allowedFile, 32);
}

export function taskFilesAllowed(task) {
  const { sections } = taskSections(task);
  return applicationFiles(sectionList(sections.find(({ name }) => name === 'files allowed'),
    'an Allowed Files list (or Files allowed)'));
}

function askContentLines(value) {
  if (typeof value !== 'string') throw new TypeError('Ask comparison requires text');
  const lines = [];
  for (const line of value.replace(/<!--[\s\S]*?-->/g, '').split(/\r?\n/)) {
    const text = line.trim();
    if (/^#{1,6}\s+task[ _-]+metadata\s*:?$/i.test(text)) break;
    if (/^(?:`{3,}|~{3,})[A-Za-z0-9_-]*$/.test(text)) continue;
    const normalized = text.replace(/^#{1,6}\s+/, '').replace(/`+/g, '')
      .replace(/\s+/g, ' ').trim();
    if (!normalized || /^(?:original[ _-]+ask|ask|description|summary|details|title)(?:\s*\([^)]*\))?\s*:?$/i.test(normalized) ||
        /^(?:task_class|difficulty|estimate_min):/i.test(normalized)) continue;
    lines.push(normalized);
  }
  return lines;
}

export function normalizeAsk(value) {
  return askContentLines(value).join(' ');
}

export function ensureOriginalAsk(task, title) {
  const required = String(title ?? '').trim();
  if (!required || normalizeAsk(task).includes(normalizeAsk(required))) return task;
  if (/^## (?:Original Ask|Ask)\s*$/im.test(task)) {
    return task.replace(/^## (?:Original Ask|Ask)\s*$/im, (heading) => `${heading}\n\n${required}`);
  }
  return `${String(task).trim()}\n\n## Original Ask\n\n${required}\n`;
}

// The issue title is authoritative; a model-written heading can borrow another issue's number from code comments.
export function ensureTitleHeading(task, title) {
  const required = String(title ?? '').trim();
  if (!required || /[\r\n]/.test(required)) return task;
  let parts;
  try {
    parts = splitTaskFrontmatter(task);
  } catch {
    return task;
  }
  const heading = /^# +(.+?)[ \t]*$/m.exec(parts.body);
  if (!heading || parts.body.slice(0, heading.index).trim()) return task;
  const text = heading[1].replace(/^(?:task|title):[ \t]*/i, '');
  if (normalizeAsk(text) === normalizeAsk(required) || /^(?:task[ \t]*)?title$|^task$/i.test(text)) return task;
  return parts.frontmatter + parts.body.slice(0, heading.index) + `# Task: ${required}` +
    parts.body.slice(heading.index + heading[0].length);
}

const namedPath = /(?:[\w.@-]+\/)+[\w.@-]+|[\w.@-]+\.(?:md|mjs|js|cjs|json|yml|yaml|txt)/g;

export function filesNamedByAsk(ask) {
  const text = String(ask ?? '');
  const window = text.match(/(?:allowed files|files allowed|only edit|edit only|do not edit any other file)[^\n]*/gi)?.join('\n') ?? text;
  return [...new Set(window.match(namedPath) ?? [])].slice(0, 32);
}

export function ensureAllowedFiles(task, ask) {
  if (/^## +(?:Allowed Files|Files allowed)\s*$/im.test(task)) return task;
  const files = filesNamedByAsk(ask);
  if (!files.length) return task;
  return `${String(task).trim()}\n\n## Allowed Files\n\n${files.map((file) => `- ${file}`).join('\n')}\n`;
}

export function ensureAcceptanceChecks(task, ask) {
  if (/^## +(?:Acceptance Checks|acceptance_checks)\s*$/im.test(task)) return task;
  const checks = String(ask ?? '').split(/\r?\n/).map((line) => line.trim())
    .filter((line) => /^[-*] /.test(line)).map((line) => line.replace(/^[-*] /, '')).slice(0, 16);
  if (!checks.length) return task;
  return `${String(task).trim()}\n\n## Acceptance Checks\n\n${checks.map((check) => `- ${check}`).join('\n')}\n`;
}

export function parseTaskDocument(task, { expectedAsk, issueTitle, issueBody } = {}) {
  const parsed = taskSections(task);
  const ask = parsed.sections.find(({ name }) => name === 'ask')?.content;
  const normalized = ask ? normalizeAsk(ask) : '';
  if (!normalized) {
    throw new TypeError('Planner TASK.md must contain a nonempty Original Ask (or Ask)');
  }
  if (expectedAsk !== undefined || issueTitle !== undefined || issueBody !== undefined) {
    const candidates = [
      issueTitle === undefined ? '' : normalizeAsk(issueTitle),
      askContentLines(issueBody ?? expectedAsk ?? '')[0] ?? '',
    ].filter(Boolean);
    if (!candidates.some((candidate) => normalized.includes(candidate))) {
      throw new TypeError('Planner TASK.md must contain the unchanged Ask title or first issue body line in Original Ask (or Ask)');
    }
  }
  const acceptance_checks = checkedList(sectionList(parsed.sections.find(({ name }) => name === 'acceptance checks'),
    'Acceptance Checks (or acceptance_checks)', { continuations: true }),
  'Acceptance checks', (line) => oneLine(line, 'Acceptance check'));
  return { title: parsed.title, ask, acceptance_checks, files_allowed: taskFilesAllowed(task) };
}
