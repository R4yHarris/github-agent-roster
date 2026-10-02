import { stripVTControlCharacters } from 'node:util';
import { formatElapsed } from './tray.mjs';

const efforts = { l: 'low', m: 'medium', h: 'high', x: 'max', none: 'none' };
const clean = (value) => {
  const text = stripVTControlCharacters(String(value ?? '')).replace(/[\x00-\x1f\x7f]/g, '').trim();
  return text || '-';
};
const count = (value) => (Number.isSafeInteger(value) && value >= 0 ? String(value) : '-');

export function formatUsage(display = {}, measured = null, { now = Date.now() } = {}) {
  const effort = measured?.effort ?? display.effort;
  const rows = [
    ['Model', clean(measured?.model ?? display.model)],
    ['Endpoint', clean(display.host)],
    ['Prompt tokens', count(measured?.input ?? display.contextUsed)],
    ['Completion tokens', count(measured?.output ?? display.outputTokens)],
    ['Context max', count(measured?.contextMax ?? display.contextMax)],
    ['Effort', clean(efforts[effort] ?? effort)],
    ['Finish reason', clean(measured?.finishReason ?? display.lastFinishReason)],
    ['Tool calls', count(display.toolCount)],
    ['Elapsed', formatElapsed(display.startedAt, now)],
    ['Thinking', display.thinking === undefined ? '-' : display.thinking ? 'enabled' : 'disabled'],
    ['Max completion tokens', count(display.maxTokens)],
  ];
  return `${rows.map(([label, value]) => `${label}: ${value}`).join('\n')}\n`;
}
