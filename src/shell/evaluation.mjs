import { splitArguments } from '../lib/arguments.mjs';
import { parseEvaluationArgs } from '../lib/eval.mjs';

export const shellEvaluationUsage = 'Use /eval SESSION accept|reject|rework 1-5 y|n --minutes N --note "what should have been done differently, and how the model performed".';

export function parseShellEvaluationArgs(text) {
  const args = splitArguments(text, shellEvaluationUsage);
  if (!args[0] || args[0].startsWith('-')) throw new TypeError(shellEvaluationUsage);
  if (!['accept', 'reject', 'rework'].includes(args[1])) throw new TypeError(shellEvaluationUsage);
  if (!/^[1-5]$/.test(args[2] ?? '')) throw new TypeError(shellEvaluationUsage);
  if (!['y', 'n'].includes(args[3])) throw new TypeError(shellEvaluationUsage);
  const options = {};
  for (let index = 4; index < args.length; index += 2) {
    const flag = args[index];
    const value = args[index + 1];
    if (!['--minutes', '--note'].includes(flag) || value === undefined || value.startsWith('--') ||
        Object.hasOwn(options, flag.slice(2))) throw new TypeError(shellEvaluationUsage);
    if (flag === '--minutes') {
      if (!/^(?:0|[1-9]\d*)$/.test(value) || !Number.isSafeInteger(Number(value))) {
        throw new TypeError(shellEvaluationUsage);
      }
      options.minutes = Number(value);
    } else options.note = value;
  }
  if (options.minutes === undefined) throw new TypeError(shellEvaluationUsage);
  if (['reject', 'rework'].includes(args[1]) && !options.note?.trim()) {
    throw new TypeError('A reject or rework requires --note before any evaluation is written.');
  }
  return {
    values: args.slice(0, 4),
    options: { minutes: options.minutes, comment: options.note ?? '', idempotent: true },
  };
}
