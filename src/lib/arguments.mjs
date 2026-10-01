export function splitArguments(input, usage = 'Use complete, whitespace-separated arguments with matching quotes.') {
  if (typeof input !== 'string') throw new TypeError(usage);
  const args = [];
  const text = input.trim();
  const token = /\s*("(?:\\.|[^"\\])*"|'[^']*'|[^\s"']+)(?=\s|$)/gy;
  while (token.lastIndex < text.length) {
    const match = token.exec(text);
    if (!match) throw new TypeError(usage);
    const value = match[1];
    try {
      args.push(value.startsWith('"') ? JSON.parse(value)
        : value.startsWith("'") ? value.slice(1, -1) : value);
    } catch {
      throw new TypeError(usage);
    }
  }
  return args;
}
