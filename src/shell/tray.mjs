import { clearLine, clearScreenDown, cursorTo, moveCursor } from 'node:readline';
import { stripVTControlCharacters } from 'node:util';
import { collectBannerFacts, formatBanner } from './banner.mjs';

const colors = { label: '\x1b[96m', white: '\x1b[97m', yellow: '\x1b[93m',
  orange: '\x1b[38;5;208m', red: '\x1b[91m', green: '\x1b[92m', reset: '\x1b[0m' };
const phases = { idle: 'idle', planning: 'plan', drafting: 'draft', testing: 'test',
  reviewing: 'review', passed: 'pass', failed: 'fail', published: 'done' };
const BAR_CELLS = 10;
const FULL_COLUMNS = 76;
const PERCENT_COLUMNS = 52;
const RULE = '\u2500';
const SEPARATOR = ' \u2502 ';

const clean = (value) => stripVTControlCharacters(String(value ?? '-')).replace(/[\x00-\x1f\x7f]/g, '');
const paint = (value, color, enabled) => (enabled ? `${colors[color]}${value}${colors.reset}` : String(value));
const length = (value) => stripVTControlCharacters(value).length;

export function formatTokens(value) {
  if (!Number.isSafeInteger(value) || value < 0) return '-';
  if (value >= 1e6) return `${(value / 1e6).toFixed(1)}m`;
  if (value >= 1000) return `${(value / 1000).toFixed(1)}k`;
  return String(value);
}

export function contextTone(used, max) {
  if (!(Number.isSafeInteger(used) && used >= 0 && Number.isSafeInteger(max) && max > 0)) return null;
  const percent = Math.min(100, (used / max) * 100);
  return percent >= 95 ? 'red' : percent >= 80 ? 'orange' : percent >= 50 ? 'yellow' : 'green';
}

export function formatContextPercent(used, max) {
  const known = Number.isSafeInteger(used) && used >= 0 && Number.isSafeInteger(max) && max > 0;
  return known ? `${Math.round(Math.min(100, (used / max) * 100))}%` : '-';
}

export function formatContext(used, max, { color = true, detail = 'full' } = {}) {
  const tone = contextTone(used, max);
  if (detail === 'percent') return tone === null ? '-' : paint(formatContextPercent(used, max), tone, color);
  if (tone === null) return `- / ${formatTokens(max)}`;
  return `${formatTokens(used)}/${formatTokens(max)} ${formatContextBar(used, max, { color })} ` +
    paint(formatContextPercent(used, max), tone, color);
}

export function formatElapsed(startedAt, now) {
  if (!Number.isFinite(startedAt)) return '-';
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m${String(seconds % 60).padStart(2, '0')}s`;
  return `${Math.floor(seconds / 3600)}h${String(Math.floor((seconds % 3600) / 60)).padStart(2, '0')}m`;
}

export function formatContextBar(used, max, { color = true } = {}) {
  const tone = contextTone(used, max);
  if (tone === null) return `[${'-'.repeat(BAR_CELLS)}]`;
  const percent = Math.min(100, (used / max) * 100);
  const filled = Math.min(BAR_CELLS, Math.round((percent / 100) * BAR_CELLS));
  return `[${paint('#'.repeat(filled), tone, color)}${'-'.repeat(BAR_CELLS - filled)}]`;
}

export function formatTray(display = {}, { columns = 80, color = true, now = Date.now(), debug = false } = {}) {
  const width = Math.max(8, Number.isSafeInteger(columns) ? columns - 1 : 79);
  const terminal = Number.isSafeInteger(columns) ? columns : 80;
  const state = clean(display.state) || 'idle';
  const phase = phases[state] ?? state;
  const issue = display.issue === null || display.issue === undefined ? 'local' : `#${display.issue}`;
  const where = state === 'idle' ? 'idle' : `${issue} ${phase}`;
  const effort = clean(display.effort ?? '-').trim() || '-';
  const id = clean(display.model).trim() || '-';
  const model = `${id} ${effort === '-' ? '-' : effort[0]}`;
  const failed = state === 'failed';
  const tail = failed && display.lastFinishReason ? clean(display.lastFinishReason)
    : state === 'idle' ? '-' : formatElapsed(display.startedAt, now);
  const detailLevel = terminal >= FULL_COLUMNS ? 'full' : terminal >= PERCENT_COLUMNS ? 'percent' : 'none';
  const context = detailLevel === 'none' ? null
    : formatContext(display.contextUsed, display.contextMax, { color, detail: detailLevel });
  const fields = [
    paint(where, failed ? 'red' : state === 'passed' ? 'green' : 'white', color),
    paint(model, 'white', color),
    ...(context === null ? [] : [context]),
    paint(tail, failed ? 'red' : 'white', color) + (debug ? paint('*', 'label', color) : ''),
  ];
  let rail = fields.join(SEPARATOR);
  for (const index of [context === null ? -1 : 2, 1]) {
    if (index < 0 || length(rail) <= width) continue;
    fields.splice(index, 1);
    rail = fields.join(SEPARATOR);
  }
  const rule = paint(RULE.repeat(width), 'label', color);
  const prompt = paint('roster> ', 'label', color);
  const detail = debug ? paint(`thinking ${display.thinking === undefined ? '-' : display.thinking ? 'on' : 'off'}` +
    `${SEPARATOR}max_tokens ${Number.isSafeInteger(display.maxTokens) ? display.maxTokens : '-'}`, 'white', color) : null;
  return { rule, rail, detail, prompt };
}

export function createTray({ output, state, shell, env = process.env, cwd = process.cwd(), services = {} }) {
  let visible = false;
  let lastRows = 1;
  let barLines = 0;
  let bannerPrinted = false;
  let bannerText = '';
  let refreshTimer;
  const frame = () => formatTray(state.display, { columns: output.columns ?? 80, debug: state.debug.enabled });

  function flush(target = output) {
    target.flush?.();
  }

  function pause() {
    if (!shell.closed) shell.pause();
  }

  function erase() {
    if (!visible) return;
    const position = shell.getCursorPos?.() ?? {};
    const rows = Number.isSafeInteger(position.rows) && position.rows > 0 ? position.rows : 0;
    cursorTo(output, 0);
    clearLine(output, 0);
    if (rows + barLines > 0) moveCursor(output, 0, -(rows + barLines));
    clearScreenDown(output);
    visible = false;
  }

  function updateRefreshTimer() {
    if (state.display.busy && refreshTimer === undefined) {
      refreshTimer = setInterval(render, 1000);
      refreshTimer.unref?.();
    } else if (!state.display.busy && refreshTimer !== undefined) {
      clearInterval(refreshTimer);
      refreshTimer = undefined;
    }
  }

  let pending = 0;

  function redraw() {
    const { rule, rail, detail, prompt } = frame();
    barLines = state.statusbar ? detail === null ? 3 : 4 : 0;
    if (barLines) output.write(`${rule}\n${rail}\n${rule}\n${detail === null ? '' : `${detail}\n`}`);
    cursorTo(output, 0);
    clearLine(output, 0);
    flush();
    shell.setPrompt(prompt);
    shell.prompt(true);
    flush();
    shell.resume();
    flush();
    visible = true;
    updateRefreshTimer();
  }

  function render() {
    if (state.pendingSecret !== null || state.pendingQuestion || shell.closed) return;
    pause();
    erase();
    redraw();
  }

  function resize() {
    if (state.pendingSecret !== null || state.pendingQuestion || shell.closed) return;
    pause();
    cursorTo(output, 0, 0);
    clearScreenDown(output);
    visible = false;
    if (bannerText) output.write(bannerText);
    redraw();
  }

  output.on?.('resize', resize);

  return {
    render,
    erase,
    async banner() {
      if (bannerPrinted) return;
      bannerPrinted = true;
      pause();
      const facts = await collectBannerFacts({ env, cwd, branch: state.display.branch,
        llm: state.config?.llm ?? {}, services });
      bannerText = formatBanner(facts);
      output.write(bannerText);
      flush();
      shell.resume();
    },
    committed() { visible = false; },
    write(text, target = output, { replace = false } = {}) {
      if (shell.closed) {
        target.write(text);
        flush(target);
        return;
      }
      pause();
      const erased = visible;
      erase();
      if (replace && erased) {
        const rows = lastRows;
        moveCursor(output, 0, -rows);
        cursorTo(output, 0);
        for (let row = 0; row < rows; row += 1) {
          clearLine(output, 0);
          if (row < rows - 1) moveCursor(output, 0, 1);
        }
        if (rows > 1) moveCursor(output, 0, -(rows - 1));
        cursorTo(output, 0);
        pending = 0;
      }
      const plain = stripVTControlCharacters(String(text));
      const width = Number.isSafeInteger(output.columns) && output.columns > 0 ? output.columns : 80;
      if (!plain.endsWith('\n')) {
        if (pending > 0) {
          moveCursor(output, 0, -1);
          if (pending >= width) {
            cursorTo(output, 0);
            moveCursor(output, 0, 1);
            pending = 0;
          } else cursorTo(output, pending);
        }
        target.write(plain);
        pending += plain.length;
        target.write('\n');
      } else {
        target.write(text);
        pending = 0;
      }
      const shown = plain.replace(/\n$/, '');
      lastRows = Math.max(1, shown.split('\n').reduce((total, line) =>
        total + Math.max(1, Math.ceil(line.length / width)), 0));
      flush(target);
      if (state.pendingSecret === null && !state.pendingQuestion) redraw();
      else {
        shell.prompt(true);
        flush();
        shell.resume();
        flush();
      }
    },
    close() {
      output.off?.('resize', resize);
      if (refreshTimer !== undefined) clearInterval(refreshTimer);
      refreshTimer = undefined;
      if (visible) {
        cursorTo(output, 0);
        clearLine(output, 0);
        output.write('\n');
      }
      visible = false;
    },
  };
}
