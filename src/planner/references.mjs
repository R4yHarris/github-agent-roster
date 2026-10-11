// References are input evidence, never local file or directory authorization.
export function withoutReferenceUrls(text) {
  const url = /(?<![A-Za-z0-9_./-])(?:[a-z][a-z0-9+.-]*:\/\/|\/\/|www\.|(?:file|data|mailto|urn):)[^\s<>]+/gi;
  const remote = (value) => new RegExp(url.source, 'i').test(value);
  const definitions = new Set();
  for (const match of text.matchAll(/^ {0,3}\[([^\]\n]+)\]:\s*(\S+)/gm)) {
    if (remote(match[2])) definitions.add(match[1].trim().toLowerCase());
  }
  const links = [];
  for (const match of text.matchAll(/!?\[[^\]\n]*\]\(/g)) {
    const start = match.index + match[0].length;
    let end = start;
    let depth = 1;
    for (; end < text.length && text[end] !== '\n' && depth; end++) {
      if (text[end] === '\\') { end++; continue; }
      if (text[end] === '(') depth++;
      if (text[end] === ')') depth--;
    }
    if (remote(text.slice(start, end))) links.push([match.index, end]);
  }
  for (const [start, end] of links.reverse()) text = text.slice(0, start) + ' ' + text.slice(end);
  return text.replace(/!?\[([^\]\n]+)\](?:\[([^\]\n]*)\])?/g,
    (span, label, id) => definitions.has((id || label).trim().toLowerCase()) ? ' ' : span)
    .replace(url, (span) => ' ' + (span.match(/[.!?]+$/)?.[0] ?? ''));
}
