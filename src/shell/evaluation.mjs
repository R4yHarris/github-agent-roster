import { splitArguments } from '../lib/arguments.mjs';
import { parseEvaluationArgs } from '../lib/eval.mjs';

export const shellEvaluationUsage = 'Use /eval SESSION accept|reject|rework 1-5 y|n --minutes N --note "what should have been done differently, and how the model performed".';

export function parseShellEvaluationArgs(text) {
  const args = splitArguments(text, shellEvaluationUsage);
  if (/^[1-5]$/.test(args[2] ?? '') && ['y', 'n'].includes(args[3])) return parseEvaluationArgs(args);
  if (args.length < 6 || !['accept', 'reject', 'rework'].includes(args[1])) {
    throw new TypeError(shellEvaluationUsage);
  }
  const options = {};
  let comment = '';
  for (let index = 2; index < args.length;) {
    const flag = args[index];
    if (!['--minutes', '--difficulty'].includes(flag)) {
      if (index !== args.length - 1 || flag.startsWith('--')) throw new TypeError(shellEvaluationUsage);
      comment = flag;
      break;
    }
    const key = flag.slice(2);
    const value = args[index + 1];
    if (Object.hasOwn(options, key) || typeof value !== 'string' ||
        (key === 'difficulty' ? !/^[1-5]$/.test(value) : !/^(?:0|[1-9]\d*)$/.test(value)) ||
        !Number.isSafeInteger(Number(value))) throw new TypeError(shellEvaluationUsage);
    options[key] = Number(value);
    index += 2;
  }
  if (options.minutes === undefined || options.difficulty === undefined) throw new TypeError(shellEvaluationUsage);
  return { values: [args[0], args[1], String(options.difficulty), 'n'], options: { minutes: options.minutes, comment } };
}
