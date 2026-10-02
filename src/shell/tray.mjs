import { clearLine, clearScreenDown, cursorTo, moveCursor } from 'node:readline';
import { stripVTControlCharacters } from 'node:util';

const colors = { label: '\x1b[96m', white: '\x1b[97m',
  red: '\x1b[91m', green: '\x1b[92m', reset: '\x1b[0m' };
const efforts = { l: 'low', m: 'medium', h: 'high', x: 'max', none: 'none' };
const clean = (value) => stripVTControlCharacters(String(value ?? '-')).replace(/[\x00-\x1f\x7f]/g, '');
const paint = (value, color, enabled) => enabled ? `${colors[color]}${value}${colors.reset}` : value;
const length = (value) => stripVTControlCharacters(value).length;

function count(value, capacity = false) {
  if (!Number.isSafeInteger(value) || value < 0 || capacity && value === 0) return '-';
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}m`;
  if (value >= 1000) return `${(value / 1000).toFixed(1)}k`;
  return String(value);
}

function elapsed(startedAt, now) {
  if (startedAt === null || startedAt === undefined) return '-';
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1000));
  return seconds >= 60 ? `${Math.floor(seconds / 60)}m ${seconds % 60}s` : `${seconds}s`;
}

function fit(text, width) {
  const value = clean(text);
  return value.length <= width ? value : width > 3 ? `${value.slice(0, width - 3)}...` : value.slice(0, width);
}

export function formatTray(display, { columns = 80, color = true, now = Date.now(), debug = false } = {}) {
  const width = Math.max(1, Number.isSafeInteger(columns) ? columns - 1 : 79);
  const issue = display.issue === null || display.issue === undefined ? 'local' : `#${display.issue}`;
  const state = clean(display.state);
  const stateColor = state === 'failed' ? 'red' : state === 'passed' ? 'green' : 'white';
  const prefix = `roster | ${issue} | ${clean(display.seat)} | ${state} | `;
  const branch = fit(display.branch || '-', Math.max(1, width - prefix.length));
  let top = `${paint('roster', 'label', color)} | ${paint(issue, 'white', color)} | ` +
    `${paint(clean(display.seat), 'white', color)} | ${paint(state, stateColor, color)} | ` +
    paint(branch, 'white', color);
  if (length(top) > width) top = paint(fit(`${prefix}${branch}`, width), stateColor, color);
  const field = (label, value) => (label ? `${paint(label, 'label', color)} ` : '') + paint(clean(value), 'white', color);
  const base = [
    field(display.mode === 'plan' ? 'plan' : '', display.model || '-'),
    field('', display.host || '-'),
    field('effort', efforts[display.effort] ?? display.effort ?? '-'),
    field('ctx', `${count(display.contextUsed)} / ${count(display.contextMax, true)}`),
    field('', elapsed(display.startedAt, now)),
    field('debug', debug ? 'on' : 'off'),
  ];
  let bottom = base.join(' | ');
  if (length(bottom) > width) {
    base.splice(4, 1);
    bottom = base.join(' | ');
  }
  if (length(bottom) > width) {
    base.splice(1, 1);
    bottom = base.join(' | ');
  }
  if (length(bottom) > width) bottom = paint(fit(bottom, width), 'white', color);
  const prompt = `${display.busy ? `${paint('*', 'label', color)} ` : ''}${paint('roster> ', 'label', color)}`;
  return { top, bottom, prompt };
}

export function createTray({ output, state, shell }) {
  let visible = false;
  let barLines = 0;
  const frame = () => formatTray(state.display, { columns: output.columns ?? 80, debug: state.debug.enabled });

  function erase() {
    if (!visible) return;
    const position = shell.getCursorPos();
    cursorTo(output, 0);
    moveCursor(output, 0, -(position.rows + barLines));
    clearScreenDown(output);
    visible = false;
  }

  function render() {
    if (state.pendingSecret !== null || state.pendingQuestion) return;
    erase();
    const { top, bottom, prompt } = frame();
    barLines = state.statusbar ? 2 : 0;
    if (barLines) output.write(`${top}\n${bottom}\n`);
    shell.setPrompt(prompt);
    shell.prompt(true);
    visible = true;
  }

  return {
    render, erase,
    banner() { output.write(`${paint('github-agent-roster', 'label', true)}\n`); },
    committed() { visible = false; },
    write(text, target = output) {
      erase();
      target.write(text);
      if (String(text).endsWith('\n')) render();
    },
    close() {
      if (visible) {
        cursorTo(output, 0);
        clearLine(output, 0);
        output.write('\n');
      }
      visible = false;
    },
  };
}
