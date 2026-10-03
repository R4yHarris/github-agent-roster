import { stripVTControlCharacters } from 'node:util';

const colors = { label: '\x1b[96m', white: '\x1b[97m', yellow: '\x1b[93m',
  red: '\x1b[91m', green: '\x1b[92m', reset: '\x1b[0m' };
const DOT = ' \u00b7 ';
const MAX = 200;

const visible = (value) => stripVTControlCharacters(String(value ?? ''))
  .replace(/[\x00-\x1f\x7f]/g, ' ').replace(/\s+/g, ' ').trim();
const clean = (value) => visible(value).slice(0, MAX);
const paint = (value, color, enabled) => (enabled && colors[color] ? `${colors[color]}${value}${colors.reset}` : value);

export function formatStep(parts) {
  const text = parts.map(clean).filter(Boolean).join(DOT);
  return text.slice(0, MAX);
}

export function createTranscript({ write, color = true } = {}) {
  let last = null;

  function emit(text, tone, { key = null, replace = false } = {}) {
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
      const delta = visible(parts.slice(previous.length));
      last = { key: 'stream', tone: 'white', count: 1, parts };
      if (!delta) return '';
      write(`${paint(delta, 'white', color)}\n`);
      return delta;
    },
    waiting(parts) {
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
