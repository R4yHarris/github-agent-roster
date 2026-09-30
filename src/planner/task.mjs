import { splitTaskFrontmatter } from '../runtime/skills.mjs';
import { isForbiddenWrite } from '../runtime/tools.mjs';

export function oneLine(value, label) {
  if (typeof value !== 'string' || !value.trim() ||
      /[\x00-\x1f\x7f]/.test(value) || value.length > 240) {
    throw new TypeError(`${label} must be one nonempty line (at most 240 characters)`);
  }
  return value.trim();
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
    throw new TypeError(`${label} must contain 1-${limit} entries`);
  }
  return items.map(check);
}

function sectionName(heading) {
  const name = heading.toLowerCase().replace(/[_-]+/g, ' ').replace(/\s+/g, ' ').replace(/:$/, '').trim();
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
  const title = oneLine(titleHeading[1].replace(/^(?:task|title):[ \t]*/i, ''), 'Task title');
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
        if (name === 'ask' && !/^original[ _-]+ask:?$/i.test(heading[2]) &&
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

function sectionList(section, label) {
  if (!section?.content) throw new TypeError(`TASK.md must contain ${label}`);
  return section.content.split('\n').filter((line) => line.trim()).map((line) => {
    const value = line.trim().replace(/^(?:[-*+]|\d+[.)])[ \t]+/, '');
    return value.replace(/^`([^`]+)`$/, '$1');
  });
}

export function taskFilesAllowed(task) {
  const { sections } = taskSections(task);
  return checkedList(sectionList(sections.find(({ name }) => name === 'files allowed'),
    'an Allowed Files list (or Files allowed)'), 'Files allowed', allowedFile, 32);
}

export function parseTaskDocument(task, { expectedAsk } = {}) {
  const parsed = taskSections(task);
  const ask = parsed.sections.find(({ name }) => name === 'ask')?.content;
  if (!ask || expectedAsk !== undefined && (typeof expectedAsk !== 'string' ||
      !expectedAsk.trim() || !ask.replace(/\s+/g, ' ').includes(expectedAsk.trim().replace(/\s+/g, ' ')))) {
    throw new TypeError('Planner TASK.md must contain the unchanged Ask in Original Ask (or Ask)');
  }
  const acceptance_checks = checkedList(sectionList(parsed.sections.find(({ name }) => name === 'acceptance checks'),
    'Acceptance Checks (or acceptance_checks)'), 'Acceptance checks', (line) => oneLine(line, 'Acceptance check'));
  return { title: parsed.title, ask, acceptance_checks, files_allowed: taskFilesAllowed(task) };
}
