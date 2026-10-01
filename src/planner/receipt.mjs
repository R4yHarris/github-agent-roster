import { applicationFiles, normalizeAsk, oneLine } from './task.mjs';
import { estimateTask } from '../runtime/estimate.mjs';

const fields = new Set(['task', 'title', 'task_class', 'difficulty', 'estimate_min', 'model', 'ask',
  'acceptance_checks', 'files_allowed', 'notes', 'steps']);
const listFields = new Set(['acceptance_checks', 'files_allowed', 'steps']);

function scalar(value) {
  if (value.startsWith('"')) {
    const parsed = JSON.parse(value);
    if (typeof parsed !== 'string') throw new TypeError('Planning receipt scalar must be text');
    return parsed;
  }
  if (value.startsWith("'") && value.endsWith("'")) return value.slice(1, -1).replaceAll("''", "'");
  if (/^(?:0|[1-9]\d*)$/.test(value)) return Number(value);
  if (!value || /[\x00-\x1f\x7f]|^[&*!{\[]/.test(value)) throw new TypeError('Invalid planning receipt scalar');
  return value;
}

export function validatePlanningReceipt(source, task) {
  const receipt = {};
  let current;
  let block = false;
  for (const original of source.replace(/\r\n/g, '\n').split('\n')) {
    if (!original.trim() || /^\s*#/.test(original)) continue;
    const header = /^([a-z_]+):\s*(.*)$/.exec(original);
    if (header) {
      const [, key, value] = header;
      if (!fields.has(key) || Object.hasOwn(receipt, key)) throw new TypeError('Invalid planning receipt field');
      current = key;
      block = ['|', '>'].includes(value);
      if (listFields.has(key)) {
        if (value) throw new TypeError('Planning receipt lists must use indented entries');
        receipt[key] = [];
      } else receipt[key] = block ? '' : scalar(value);
      continue;
    }
    const item = /^\s{2,}-\s+(.+)$/.exec(original);
    if (item && listFields.has(current)) receipt[current].push(scalar(item[1]));
    else if (block && /^\s{2,}\S/.test(original)) receipt[current] += `${original.trim()}\n`;
    else throw new TypeError('Invalid planning receipt YAML');
  }
  const title = receipt.title ?? receipt.task;
  if (typeof title !== 'string' || normalizeAsk(title) !== normalizeAsk(task.title) ||
      !Array.isArray(receipt.acceptance_checks) || !receipt.acceptance_checks.length ||
      !Array.isArray(receipt.files_allowed)) {
    throw new TypeError('Planning receipt must match the task title and contain checks and allowed files');
  }
  receipt.acceptance_checks.forEach((check) => oneLine(check, 'Receipt acceptance check'));
  const files = applicationFiles(receipt.files_allowed);
  if (files.length !== task.files_allowed.length || files.some((file) => !task.files_allowed.includes(file))) {
    throw new TypeError('Planning receipt application paths differ from TASK.md');
  }
  estimateTask(receipt);
}
