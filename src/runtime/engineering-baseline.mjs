import { readFileSync } from 'node:fs';

export function readEngineeringBaseline() {
  const text = readFileSync(new URL('../../principals/engineering.md', import.meta.url), 'utf8')
    .replace(/\r\n/g, '\n').trim();
  if (!text || text.length > 8192) throw new Error('Engineering baseline must contain 1-8192 characters');
  return text;
}

export function withEngineeringBaseline(messages) {
  if (!Array.isArray(messages) || !messages.length) {
    throw new TypeError('Engineering baseline requires nonempty chat messages');
  }
  const baseline = readEngineeringBaseline();
  const first = messages[0];
  if (first?.role === 'system' && typeof first.content === 'string') {
    if (first.content.includes(baseline)) return messages;
    return [{ ...first, content: `${first.content}\n\n${baseline}` }, ...messages.slice(1)];
  }
  return [{ role: 'system', content: baseline }, ...messages];
}
