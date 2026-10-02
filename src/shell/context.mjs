import { stripVTControlCharacters } from 'node:util';
import { redactEvidence } from '../runtime/excellence.mjs';

export function formatContext(measured, { packBudgetChars, env = process.env } = {}) {
  const safe = (value) => stripVTControlCharacters(redactEvidence(String(value ?? '-'), { env }))
    .replace(/[\x00-\x1f\x7f]/g, '?').slice(0, 240);
  const count = (value) => {
    if (value === undefined || value === null) return '-';
    if (!Number.isSafeInteger(value) || value < 0) throw new TypeError('Context readout count is invalid');
    return String(value);
  };
  const included = measured?.priorFeedbackIncluded;
  if (included !== undefined && typeof included !== 'boolean') throw new TypeError('Context feedback metadata is invalid');
  return `Seat: ${safe(measured?.seat)}\nProvider: ${safe(measured?.provider)}\nModel: ${safe(measured?.model)}\n` +
    `Effort: ${safe(measured?.effort)}\nInput: ${count(measured?.input)}\nOutput: ${count(measured?.output)}\n` +
    `Context max: ${count(measured?.contextMax)}\nFinish reason: ${safe(measured?.finishReason)}\n` +
    `Pack budget (characters): ${count(measured?.packBudgetChars ?? packBudgetChars)}\n` +
    `Prior feedback included: ${included === undefined ? '-' : included ? 'yes' : 'no'}\n`;
}
