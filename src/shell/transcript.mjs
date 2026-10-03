import { stripVTControlCharacters } from 'node:util';

const colors = { label: '\x1b[96m', white: '\x1b[97m', yellow: '\x1b[93m',
  red: '\x1b[91m', green: '\x1b[92m', reset: '\x1b[0m' };
const DOT = ' \u00b7 ';
const MAX = 200;

const raw = (value) => stripVTControlCharacters(String(value ?? '')).replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g, '');
const clean = (value) => raw(value).replace(/\s+/g, ' ').trim().slice(0, MAX);
const paint = (value, color, enabled) => (enabled && colors[color] ? `${colors[color]}${value}${colors.reset}` : value);

export function formatStep(parts) {
  const text = parts.map(clean).filter(Boolean).join(DOT);
  return text.slice(0, MAX);
}

export function wrapNarration(text, columns = 80) {
  const width = Math.max(20, Number.isSafeInteger(columns) && columns > 1 ? columns - 1 : 79);
  const lines = [];
  for (const paragraph of String(text ?? '').split('\n')) {
    let rest = paragraph;
    if (!rest) {
      lines.push('');
      continue;
    }
    while (rest.length > width) {
      const space = rest.lastIndexOf(' ', width);
      const cut = space >= Math.floor(width / 2) ? space : width;
      lines.push(rest.slice(0, cut));
      rest = rest.slice(cut).replace(/^ /, '');
    }
    lines.push(rest);
  }
  return lines;
}

export function createTranscript({ write, color = true, columns = () => 80 } = {}) {
  let last = null;

  function closeStream() {
    if (last?.key !== 'stream') return;
    last = null;
  }

  function emit(text, tone, { key = null, replace = false } = {}) {
    closeStream();
    write(`${paint(text, tone, color)}\n`, { replace });
    last = key === null ? null : { key, tone, count: 1, parts: null };
    return text;
  }

  return {
    reset() { last = null; },
    get last() { return last; },
    line(parts, { tone = 'white' } = {}) {
      return emit(formatStep([].concat(parts)), tone);
    },
    phase(issue, name) {
      const label = issue === null || issue === undefined ? 'local' : `#${issue}`;
      return emit(formatStep([`${label} ${clean(name)}`.trim()]), 'white');
    },
    tool(name, target, total) {
      closeStream();
      const label = `${clean(name)} ${clean(target)}`.trim();
      const key = `tool:${label}`;
      const same = Boolean(last && last.key === key);
      const explicit = Number.isSafeInteger(total) && total >= 1;
      const count = explicit ? total : same ? last.count + 1 : 1;
      const text = formatStep([label, count > 1 ? String(count) : '']);
      write(`${paint(text, 'white', color)}\n`, { replace: same });
      last = { key, tone: 'white', count };
      return text;
    },
    stream(text, { start = false } = {}) {
      const open = Boolean(last && last.key === 'stream') && !start;
      const incoming = String(text ?? '');
      const previous = open ? last.parts : '';
      const parts = incoming === previous || incoming.startsWith(previous) ? incoming : `${previous}${incoming}`;
      const lines = wrapNarration(raw(parts), columns());
      last = { key: 'stream', tone: 'white', count: 1, parts };
      write(`${paint(lines.join('\n'), 'white', color)}\n`, { replace: open });
      return lines.join('\n');
    },
    waiting(parts) {
      closeStream();
      const text = formatStep(['waiting'].concat(parts));
      const replace = Boolean(last && last.key === 'waiting');
      write(`${paint(text, 'yellow', color)}\n`, { replace });
      last = { key: 'waiting', tone: 'yellow', count: 1 };
      return text;
    },
    verdict(parts, { pass = false } = {}) {
      return emit(formatStep([].concat(parts)), pass ? 'green' : 'red');
    },
  };
}
