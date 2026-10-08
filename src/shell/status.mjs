import { stripVTControlCharacters } from 'node:util';
import { redactSecrets } from '../runtime/memory.mjs';

export function formatSessionStatus(state, { env = process.env } = {}) {
  const display = state.display;
  const safe = (value) => stripVTControlCharacters(redactSecrets(String(value ?? '-'), { env }))
    .replace(/[\x00-\x1f\x7f]/g, '?').slice(0, 240);
  const effort = { l: 'low', m: 'medium', h: 'high', x: 'max' }[display.effort] ?? display.effort;
  return `Issue: ${display.issue === null ? 'local' : `#${display.issue}`}\n` +
    `Branch: ${safe(display.branch)}\nSeat: ${safe(display.seat)}\nState: ${safe(display.state)}\n` +
    `Model: ${safe(display.model || '-')}\nHost: ${safe(display.host || '-')}\nEffort: ${safe(effort)}\n` +
    `Last finish reason: ${safe(display.lastFinishReason)}\nLast test name: ${safe(display.lastTestName)}\n` +
    `Review: ${safe(display.review)}\n` +
    (display.waves?.length ? 'Child waves:\n' + display.waves.map((row) =>
      `  #${row.issue} wave:${row.wave} ${safe(row.seat)} ${safe(row.state)} ${safe(row.model)}\n`).join('') : '') +
    (state.lastRun?.worktreePath ? `Worktree: ${safe(state.lastRun.worktreePath)}\n` : '');
}
