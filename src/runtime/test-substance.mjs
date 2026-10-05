// Deterministic, model-independent checks that new tests exercise application code.
// Heuristic by design: it flags only clear patterns (tests that never call app code,
// or seeded sentinels asserted absent without ever reaching app code).

const testHarnessSpecifiers = new Set(['node:assert', 'node:assert/strict', 'assert', 'assert/strict', 'node:test']);
const keywords = new Set(['if', 'for', 'while', 'switch', 'catch', 'function', 'return', 'typeof', 'new', 'await',
  'test', 'it', 'describe', 'assert', 'JSON', 'Object', 'Array', 'String', 'Number', 'Date', 'Math', 'Promise']);

export function isTestFile(file) {
  const normalized = file.replaceAll('\\', '/');
  return /\.(test|spec)\.[cm]?[jt]s$/.test(normalized) ||
    /(^|\/)(tests?|__tests__)\/.+\.[cm]?[jt]s$/.test(normalized);
}

// Blank out comments and string/template/regex contents while keeping offsets stable.
function maskCode(text) {
  let out = '';
  let index = 0;
  let previous = '';
  while (index < text.length) {
    const char = text[index];
    const next = text[index + 1];
    if (char === '/' && next === '/') {
      while (index < text.length && text[index] !== '\n') { out += ' '; index += 1; }
      continue;
    }
    if (char === '/' && next === '*') {
      while (index < text.length && !(text[index] === '*' && text[index + 1] === '/')) {
        out += text[index] === '\n' ? '\n' : ' '; index += 1;
      }
      out += '  '; index += 2;
      continue;
    }
    if (char === '"' || char === "'" || char === '`' ||
        (char === '/' && /[(,=:[!&|?{};]$|^$/.test(previous))) {
      const quote = char;
      out += quote; index += 1;
      while (index < text.length && text[index] !== quote) {
        if (text[index] === '\\') { out += '  '; index += 2; continue; }
        if (quote !== '`' && text[index] === '\n') break;
        out += text[index] === '\n' ? '\n' : ' '; index += 1;
      }
      if (index < text.length) { out += text[index]; index += 1; }
      previous = quote;
      continue;
    }
    out += char;
    if (!/\s/.test(char)) previous = char;
    index += 1;
  }
  return out;
}

function matchingClose(masked, open) {
  const pairs = { '(': ')', '[': ']', '{': '}' };
  const stack = [];
  for (let index = open; index < masked.length; index += 1) {
    const char = masked[index];
    if (pairs[char]) stack.push(pairs[char]);
    else if (char === ')' || char === ']' || char === '}') {
      if (stack.pop() !== char) return -1;
      if (!stack.length) return index;
    }
  }
  return -1;
}

// End of the statement starting at index: first `;` or newline at depth 0 after content.
function statementEnd(masked, index) {
  let depth = 0;
  for (let cursor = index; cursor < masked.length; cursor += 1) {
    const char = masked[cursor];
    if ('([{'.includes(char)) depth += 1;
    else if (')]}'.includes(char)) {
      depth -= 1;
      if (depth < 0) return cursor;
    } else if (depth === 0 && (char === ';' || char === '\n')) {
      const rest = masked.slice(cursor + 1).match(/^\s*(\S)/);
      if (char === '\n' && rest && /[.?:+\-*/|&,=)\]}]/.test(rest[1])) continue;
      return cursor;
    }
  }
  return masked.length;
}

function parseImports(text) {
  const imports = [];
  const pattern = /^\s*import\s+([\s\S]*?)\s+from\s+['"]([^'"]+)['"]/gm;
  for (const match of text.matchAll(pattern)) {
    const [, clause, specifier] = match;
    const names = [];
    const namespace = /\*\s+as\s+([A-Za-z_$][\w$]*)/.exec(clause);
    if (namespace) names.push({ name: namespace[1], namespace: true });
    const named = /\{([^}]*)\}/.exec(clause);
    if (named) {
      for (const part of named[1].split(',')) {
        const local = part.trim().split(/\s+as\s+/).pop()?.trim();
        if (local && /^[A-Za-z_$][\w$]*$/.test(local)) names.push({ name: local });
      }
    }
    const fallback = /^([A-Za-z_$][\w$]*)/.exec(clause.trim());
    if (fallback && !clause.trim().startsWith('{') && !clause.trim().startsWith('*')) names.push({ name: fallback[1] });
    const harness = testHarnessSpecifiers.has(specifier);
    // Spawning a CLI is black-box app coverage.
    const app = !harness && (/^(node:)?child_process$/.test(specifier) ||
      !specifier.startsWith('node:') && !/^(fs|path|os|url|util|crypto)(\/|$)/.test(specifier));
    for (const entry of names) imports.push({ ...entry, specifier, harness, app });
  }
  return imports;
}

function callPattern(names) {
  const escaped = [...names].map((name) => name.replace(/\$/g, '\\$'));
  return escaped.length ? new RegExp(`(?<![\\w$.])(?:new\\s+)?(${escaped.join('|')})(?:\\s*\\.\\s*[A-Za-z_$][\\w$]*)*\\s*\\(`, 'g') : null;
}

function calls(masked, names) {
  const pattern = callPattern(names);
  if (!pattern) return [];
  const found = [];
  for (const match of masked.matchAll(pattern)) {
    const open = match.index + match[0].length - 1;
    const close = matchingClose(masked, open);
    found.push({ name: match[1], start: match.index, open, close: close < 0 ? masked.length : close });
  }
  return found;
}

// File-local functions whose bodies call app code (directly or through another such helper).
function appHelpers(text, masked, appNames) {
  const definitions = [];
  const pattern = /(?:^|[^\w$.])(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)\s*\(|(?:^|[^\w$.])(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s+)?(?:function\b|\([^)]*\)\s*=>|[A-Za-z_$][\w$]*\s*=>)/g;
  for (const match of masked.matchAll(pattern)) {
    const name = match[1] ?? match[2];
    const start = match.index;
    let end;
    if (match[1]) {
      const brace = masked.indexOf('{', masked.indexOf(')', start));
      end = brace < 0 ? masked.length : matchingClose(masked, brace);
    } else end = statementEnd(masked, masked.indexOf('=', start) + 1);
    definitions.push({ name, body: masked.slice(start, end < 0 ? masked.length : end + 1) });
  }
  const helpers = new Set();
  let changed = true;
  while (changed) {
    changed = false;
    const sinks = new Set([...appNames, ...helpers]);
    for (const { name, body } of definitions) {
      if (helpers.has(name) || appNames.has(name)) continue;
      if (calls(body.replace(new RegExp(`^[\\s\\S]*?${name.replace(/\$/g, '\\$')}`), ''), sinks).length) {
        helpers.add(name);
        changed = true;
      }
    }
  }
  return helpers;
}

function addedTestBlocks(added, maskedAdded) {
  const blocks = [];
  for (const match of maskedAdded.matchAll(/(?<![\w$.])(test|it)(?:\.only|\.skip|\.todo)?\s*\(/g)) {
    const open = match.index + match[0].length - 1;
    const close = matchingClose(maskedAdded, open);
    if (close < 0) continue;
    const title = /^\s*(['"`])([^'"`\n]{1,200})\1/.exec(added.slice(open + 1, close))?.[2] ?? '(untitled)';
    blocks.push({ title, start: open, end: close });
  }
  return blocks;
}

function stringLiterals(text, masked) {
  const literals = [];
  for (let index = 0; index < masked.length; index += 1) {
    const quote = masked[index];
    if (quote !== '"' && quote !== "'" && quote !== '`') continue;
    const end = masked.indexOf(quote, index + 1);
    if (end < 0) break;
    const value = text.slice(index + 1, end);
    if (!value.includes('${')) literals.push({ value, start: index, end });
    index = end;
  }
  return literals;
}

const absencePattern = /doesNotMatch\s*\(|notEqual\s*\(|notStrictEqual\s*\(|!\s*[\w$.()[\]'"`-]*\.includes\s*\(|\.includes\s*\([^\n]*\)\s*,\s*false\b|\.includes\s*\([^\n]*\)\s*===?\s*false\b|\.indexOf\s*\([^\n]*\)\s*===?\s*-1\b|\.(?:not\.)?(?:toContain|toMatch)\b/;

function regexBodies(line) {
  return [...line.matchAll(/\/((?:\\.|[^/\n\\])+)\/[a-z]*/g)].map((match) => match[1].replace(/\\(.)/g, '$1'));
}

// Sentinels: literals (>= 6 chars) seeded in added code and referenced by an absence assertion.
function seededSentinels(added, maskedAdded) {
  const lines = added.split('\n');
  const maskedLines = maskedAdded.split('\n');
  const titles = new Set(addedTestBlocks(added, maskedAdded)
    .map(({ start }) => start + 1 + (/^\s*/.exec(maskedAdded.slice(start + 1))?.[0].length ?? 0)));
  const literals = stringLiterals(added, maskedAdded)
    .filter(({ value, start }) => value.length >= 6 && !titles.has(start));
  const bound = new Map();
  for (const match of maskedAdded.matchAll(/(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(['"`])/g)) {
    const start = match.index + match[0].length - 1;
    const literal = literals.find((entry) => entry.start === start);
    if (literal && /^\s*[;\n]/.test(maskedAdded.slice(literal.end + 1, literal.end + 3) || ';')) bound.set(match[1], literal.value);
  }
  const sentinels = new Map();
  let offset = 0;
  for (const [index, line] of lines.entries()) {
    const masked = maskedLines[index];
    const lineStart = offset;
    offset += line.length + 1;
    if (!absencePattern.test(masked)) continue;
    const lineEnd = lineStart + line.length;
    const inLine = literals.filter(({ start }) => start >= lineStart && start < lineEnd).map(({ value }) => ({ value, exact: true }));
    const candidates = [...inLine, ...regexBodies(line).filter((body) => body.length >= 6).map((value) => ({ value, exact: false }))];
    for (const [name, value] of bound) {
      if (new RegExp(`(?<![\\w$])(?<![^.]\\.)${name.replace(/\$/g, '\\$')}(?![\\w$])`).test(masked)) sentinels.set(value, name);
    }
    for (const candidate of candidates) {
      const seeded = literals.find(({ value, start }) => (start < lineStart || start >= lineEnd) &&
        (candidate.exact ? value === candidate.value : value.includes(candidate.value)));
      if (seeded && !sentinels.has(seeded.value)) sentinels.set(seeded.value, null);
    }
  }
  return sentinels;
}

function sentinelReached(added, maskedAdded, value, sinkNames) {
  const literalRanges = stringLiterals(added, maskedAdded).filter((entry) => entry.value.includes(value) || value.includes(entry.value) && entry.value.length >= 6);
  const tainted = new Set();
  const containsTaint = (start, end) => literalRanges.some((range) => range.start >= start && range.start < end) ||
    [...tainted].some((name) => new RegExp(`(?<![\\w$'"])(?<![^.]\\.)${name.replace(/\$/g, '\\$')}(?![\\w$])`).test(maskedAdded.slice(start, end)));
  const declarations = [];
  for (const match of maskedAdded.matchAll(/(?:const|let|var)\s+([A-Za-z_$][\w$]*|\{[^}]*\}|\[[^\]]*\])\s*=/g)) {
    const names = match[1].match(/[A-Za-z_$][\w$]*/g)?.filter((name) => !keywords.has(name)) ?? [];
    const start = match.index + match[0].length;
    declarations.push({ names, start, end: statementEnd(maskedAdded, start) });
  }
  let changed = true;
  while (changed) {
    changed = false;
    for (const { names, start, end } of declarations) {
      if (names.every((name) => tainted.has(name)) || !containsTaint(start, end)) continue;
      for (const name of names) tainted.add(name);
      changed = true;
    }
  }
  for (const match of maskedAdded.matchAll(/process\.env(?:\.[A-Za-z_$][\w$]*|\[[^\]]+\])\s*=(?!=)/g)) {
    const start = match.index + match[0].length;
    if (containsTaint(start, statementEnd(maskedAdded, start))) return true;
  }
  return calls(maskedAdded, sinkNames).some(({ open, close }) => containsTaint(open, close));
}

// True when added test-file lines contain a new test/it block or an assertion.
export function addsTestEvidence(added) {
  if (typeof added !== 'string' || !added.trim()) return false;
  const maskedAdded = maskCode(added);
  return addedTestBlocks(added, maskedAdded).length > 0 ||
    /(?<![\w$])(?:assert(?:\.[A-Za-z]+)?|expect|t\.assert\.[A-Za-z]+)\s*\(/.test(maskedAdded);
}

// True when added lines are only import statements, comments, hunk separators, or whitespace.
export function addsOnlyImports(added) {
  if (typeof added !== 'string' || !added.trim()) return false;
  return !added
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/^\s*\/\/.*$/gm, '')
    .replace(/\bimport\s+(?:[\w$*{}\s,]+?\s+from\s+)?(['"])[^'"\n]+\1\s*;?/g, '')
    .replace(/^\s*;\s*$/gm, '')
    .trim();
}

export function analyzeTestSubstance({ file, text, added }) {
  if (typeof added !== 'string' || !added.trim()) return [];
  const imports = parseImports(text);
  const appNames = new Set(imports.filter(({ app }) => app).map(({ name }) => name));
  const sinkNames = new Set(imports.filter(({ harness }) => !harness).map(({ name }) => name));
  const masked = maskCode(text);
  const helpers = appHelpers(text, masked, appNames);
  const appSinks = new Set([...appNames, ...helpers]);
  const allSinks = new Set([...sinkNames, ...helpers]);
  const maskedAdded = maskCode(added);
  const reasons = [];
  // Only demand app calls when the file already imports an app seam the test could use.
  for (const block of appNames.size ? addedTestBlocks(added, maskedAdded) : []) {
    const body = maskedAdded.slice(block.start, block.end + 1);
    if (!calls(body, appSinks).length) {
      reasons.push(`Test substance: new test "${block.title.slice(0, 80)}" in ${file} never calls imported app code; ` +
        'its assertions only inspect values the test built, so they cannot catch a regression.');
    }
  }
  for (const [value, name] of seededSentinels(added, maskedAdded)) {
    if (sentinelReached(added, maskedAdded, value, allSinks)) continue;
    const label = name ?? `'${value.slice(0, 40)}'`;
    reasons.push(`Test substance: sentinel ${label} in ${file} is asserted absent but never passed to app code ` +
      '(arguments, config, or process.env); the assertion cannot fail.');
  }
  return reasons;
}

// Added lines per file from a `git diff --unified=0` patch.
export function addedLinesByFile(diff) {
  const files = new Map();
  let current = null;
  for (const line of diff.split('\n')) {
    const header = /^diff --git a\/(.+) b\/(.+)$/.exec(line);
    if (header) {
      current = header[2];
      files.set(current, []);
      continue;
    }
    if (!current) continue;
    if (line.startsWith('@@')) files.get(current).push(';');
    else if (line.startsWith('+') && !line.startsWith('+++')) files.get(current).push(line.slice(1));
  }
  return new Map([...files].map(([file, lines]) => [file, lines.join('\n')]));
}
