import { stripVTControlCharacters } from 'node:util';

const colors = { label: '\x1b[96m', white: '\x1b[97m', yellow: '\x1b[93m',
  red: '\x1b[91m', green: '\x1b[92m', reset: '\x1b[0m' };
const DOT = ' \u00b7 ';
const MAX = 200;

const clean = (value) => stripVTControlCharacters(String(value ?? ''))
  .replace(/[\x00-\x1f\x7f]/g, ' ').replace(/\s+/g, ' ').trim().slice(0, MAX);
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
    tool(name, target) {
      const key = `tool:${clean(name)}:${clean(target)}`;
      if (last && last.key === key) {
        last.count += 1;
        write(`${paint(formatStep([`${clean(name)} ${clean(target)}`.trim(), String(last.count)]), 'white', color)}\n`,
          { replace: true });
        return formatStep([`${clean(name)} ${clean(target)}`.trim(), String(last.count)]);
      }
      return emit(formatStep([`${clean(name)} ${clean(target)}`.trim()]), 'white', { key });
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
