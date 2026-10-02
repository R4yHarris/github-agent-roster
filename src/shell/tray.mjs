import { clearLine, clearScreenDown, cursorTo, moveCursor } from 'node:readline';
import { stripVTControlCharacters } from 'node:util';
import { collectBannerFacts, formatBanner } from './banner.mjs';

const colors = { label: '\x1b[96m', white: '\x1b[97m', yellow: '\x1b[93m',
  orange: '\x1b[38;5;208m', red: '\x1b[91m', green: '\x1b[92m', reset: '\x1b[0m' };
const phases = { idle: 'idle', planning: 'plan', drafting: 'draft', testing: 'test',
  reviewing: 'review', passed: 'pass', failed: 'fail', published: 'done' };
const BAR_CELLS = 10;
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

export function formatContext(used, max, { color = true } = {}) {
  const known = Number.isSafeInteger(used) && used >= 0 && Number.isSafeInteger(max) && max > 0;
  const text = `${known ? formatTokens(used) : '-'} / ${formatTokens(max)}`;
  return known ? `${formatContextBar(used, max, { color })} ${text}` : text;
}

export function formatElapsed(startedAt, now) {
  if (!Number.isFinite(startedAt)) return '-';
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m${String(seconds % 60).padStart(2, '0')}s`;
  return `${Math.floor(seconds / 3600)}h${String(Math.floor((seconds % 3600) / 60)).padStart(2, '0')}m`;
}

export function formatContextBar(used, max, { color = true } = {}) {
  const known = Number.isSafeInteger(used) && used >= 0 && Number.isSafeInteger(max) && max > 0;
  if (!known) return `[${'-'.repeat(BAR_CELLS)}]`;
  const percent = Math.min(100, (used / max) * 100);
  const filled = Math.min(BAR_CELLS, Math.max(percent > 0 ? 1 : 0, Math.round((percent / 100) * BAR_CELLS)));
  const tone = percent >= 95 ? 'red' : percent >= 80 ? 'orange' : percent >= 50 ? 'yellow' : 'green';
  return `[${paint('#'.repeat(filled), tone, color)}${'-'.repeat(BAR_CELLS - filled)}]`;
}

export function formatTray(display = {}, { columns = 80, color = true, now = Date.now(), debug = false } = {}) {
  const width = Math.max(8, Number.isSafeInteger(columns) ? columns - 1 : 79);
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
  const fields = [
    paint(where, failed ? 'red' : state === 'passed' ? 'green' : 'white', color),
    paint(model, 'white', color),
    formatContext(display.contextUsed, display.contextMax, { color }),
    paint(tail, failed ? 'red' : 'white', color) + (debug ? paint('*', 'label', color) : ''),
  ];
  let rail = fields.join(SEPARATOR);
  for (const index of [2, 1]) {
    if (length(rail) <= width) break;
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
  let barLines = 0;
  let bannerPrinted = false;
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
    visible = true;
    updateRefreshTimer();
  }

  function render() {
    if (state.pendingSecret !== null || state.pendingQuestion || shell.closed) return;
    pause();
    erase();
    redraw();
  }

  return {
    render,
    erase,
    async banner() {
      if (bannerPrinted) return;
      bannerPrinted = true;
      pause();
      const facts = await collectBannerFacts({ env, cwd, branch: state.display.branch,
        llm: state.config?.llm ?? {}, services });
      output.write(formatBanner(facts));
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
        moveCursor(output, 0, -1);
        cursorTo(output, 0);
        clearLine(output, 0);
      }
      target.write(text);
      flush(target);
      if (String(text).endsWith('\n') && state.pendingSecret === null && !state.pendingQuestion) redraw();
      else shell.resume();
    },
    close() {
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
