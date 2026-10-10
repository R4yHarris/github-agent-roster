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

// A destination directive grants a directory, not paths mentioned in explanatory prose.
export function outputDirectoryFromAsk(ask) {
  if (/^#{1,3} (?:Files allowed|Allowed files|files_allowed|allowed_files)\s*$/im.test(String(ask ?? ''))) {
    return undefined;
  }
  const directive = /(?:^|[.!?\n]\s*)(?:output|save|put|place)\s+(?:(?:it|them|the (?:output|documents|files))\s+)?(?:in|into|to)\s+(?:(?:a|an|the)\s+)?(?:new\s+)?(?:"([^"\n]+)"|'([^'\n]+)'|`([^`\n]+)`|([^\s"'`]+))\s+directory\b/gi;
  const text = String(ask ?? '').replace(/<!--[\s\S]*?-->/g, '')
    .replace(/^ {0,3}(`{3,}|~{3,})[^\n]*\n[\s\S]*?^ {0,3}\1[ \t]*$/gm, '')
    .replace(/^[ \t]*>.*$/gm, '')
    .replace(/(["'`])((?:(?!\1)[^\n])*)\1/g, (span, _quote, content) =>
      /\b(?:output|save|put|place)\s+/i.test(content) ? '' : span);
  const directories = [...text.matchAll(directive)].map((match) => {
    const directory = match.slice(1).find((value) => value !== undefined);
    if (!/^[A-Za-z0-9_-][A-Za-z0-9_.-]*(?:\/[A-Za-z0-9_-][A-Za-z0-9_.-]*)*$/.test(directory) ||
        directory.split('/').some((part) => part.endsWith('.') || part.toLowerCase() === 'vendor') ||
        isForbiddenWrite(directory) || isForbiddenWrite(`${directory}/output.md`)) {
      throw new TypeError('Output directory must be relative, bounded, and exclude protected paths');
    }
    return directory;
  });
  if (new Set(directories).size > 1) throw new TypeError('Ask must grant one output directory');
  return directories[0];
}

export function validateDirectoryFiles(files, directory) {
  const fold = (value) => process.platform === 'win32' ? value.toLowerCase() : value;
  if (files.some((file) => {
    allowedFile(file);
    return file.includes('*') || !fold(file).startsWith(`${fold(directory)}/`) ||
      file.split('/').some((part) => part.startsWith('.') || part.toLowerCase() === 'vendor');
  })) throw new TypeError('Planner must choose concrete, unprotected files inside the granted output directory');
}

export function isDirectoryDocumentAsk(ask) {
  if (!outputDirectoryFromAsk(ask)) return false;
  const text = String(ask).trim();
  const documentIntent = /\b(?:docs?|documentation|documents?|designs?|setup|set-up|instructions?|guides?|manuals?|specifications?|reports?|proposals?|readme)\b/i;
  if (!documentIntent.test(text)) return false;
  // Documentation language fails closed unless the human explicitly requests an implementation artifact.
  const implementation = /(?:^|[.!?\n]\s*)(?:please\s+)?(?:implement|build|develop|code|write|generate|create)\s+(?:me\s+)?(?:(?:a|an|the|new|simple|working|executable|static|shell|node|python)\s+)*(?:script|program|application|app|code|website|web page)\b/i;
  return !implementation.test(text);
}

export function validateDirectoryDocumentPlan(ask, files, checks) {
  if (!isDirectoryDocumentAsk(ask)) return;
  if (files.some((file) => !/\.md$/i.test(file))) {
    throw new TypeError('Directory documentation asks authorize Markdown documents, not executable or deployment artifacts');
  }
  const commands = /\b(?:npm|npx|pnpm|yarn|pip|apt|docker|kubectl|terraform|curl|wget|ssh|bash|powershell|Invoke-WebRequest)\b|\b(?:git\s+(?:push|clone)|gh\s+(?:pr|release)|node\s+--test)\b/i;
  const action = /\b(?:install(?:ation|ed|s)?|deploy(?:ment|ed|s)?|publish(?:ed|es|ing)?|purchase(?:d|s)?|buy|account|network|request|fetch|download(?:ed|s)?|upload(?:ed|s)?|serve(?:d|s)?|hosting)\b/i;
  const assertion = /\b(?:run|execute|succeeds?|successful(?:ly)?|exits?|passes?|created|activated|completed|live|reachable)\b|\b(?:is|are|was|were)\s+(?:installed|deployed|published|purchased|serving|running)\b/i;
  const content = /\b(?:documents?|describes?|explains?|outlines?|contains?|includes?|covers?|lists?|mentions?|states?|warns?)\b/i;
  const activeClause = /(?:[;:]|\b(?:and|then))\s+(?:run|execute|install|deploy|publish|purchase|buy|register|create an? account|fetch|download|upload|request)(?:s|ed)?\b/i;
  for (const check of checks) {
    if (!content.test(check) || activeClause.test(check) || commands.test(check) && assertion.test(check) ||
        action.test(check) && assertion.test(check)) {
      throw new TypeError('Directory documentation requires document-content checks, not installation, deployment, publication, network, purchase or account actions');
    }
  }
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
      rawContent: text.slice(section.contentStart, end),
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
  const askSection = parsed.sections.find(({ name }) => name === 'ask');
  let ask = askSection?.content;
  const normalized = ask ? normalizeAsk(ask) : '';
  if (!normalized) {
    throw new TypeError('Planner TASK.md must contain a nonempty Original Ask (or Ask)');
  }
  const authoritativeAsk = expectedAsk ?? issueBody;
  const outputDirectory = outputDirectoryFromAsk(authoritativeAsk ?? ask);
  if (outputDirectory) {
    const originalHeading = /^#{2,6} +Original[ _-]+Ask\b/i.test(askSection.source);
    const raw = askSection.rawContent;
    // Canonical ## Ask has one structural final newline; human-style ## Original Ask has blank separators.
    ask = originalHeading && raw.startsWith('\n') && raw.endsWith('\n\n')
      ? raw.slice(1, -2) : raw.endsWith('\n') ? raw.slice(0, -1) : raw;
    if (authoritativeAsk !== undefined && ask !== String(authoritativeAsk).replace(/\r\n/g, '\n')) {
      throw new TypeError('Directory-scoped TASK.md must preserve the unchanged original Ask');
    }
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
  const files_allowed = taskFilesAllowed(task);
  if (outputDirectory) {
    validateDirectoryFiles(files_allowed, outputDirectory);
    validateDirectoryDocumentPlan(authoritativeAsk ?? ask, files_allowed, acceptance_checks);
    if (files_allowed.every((file) => file.endsWith('.md')) &&
        acceptance_checks.every((check) => /node --test/.test(check))) {
      throw new TypeError('Directory documentation requires document-content acceptance checks');
    }
  }
  return { title: parsed.title, ask, acceptance_checks, files_allowed };
}
