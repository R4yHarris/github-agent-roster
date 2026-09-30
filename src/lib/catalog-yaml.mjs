function uncomment(line) {
  let quoted = false;
  let escaped = false;
  for (let index = 0; index < line.length; index += 1) {
    const char = line[index];
    if (quoted && char === '\\' && !escaped) {
      escaped = true;
      continue;
    }
    if (char === '"' && !escaped) quoted = !quoted;
    if (!quoted && char === '#' && (index === 0 || /\s/.test(line[index - 1]))) {
      return line.slice(0, index).trimEnd();
    }
    escaped = false;
  }
  return line.trimEnd();
}

function scalar(text, location) {
  if (text.startsWith('"')) {
    try {
      const value = JSON.parse(text);
      if (typeof value === 'string') return value;
    } catch {
      throw new TypeError(`${location}: invalid double-quoted string`);
    }
    throw new TypeError(`${location}: expected a string`);
  }
  if (/^(?:0|[1-9]\d*)$/.test(text)) {
    const value = Number(text);
    if (!Number.isSafeInteger(value)) throw new TypeError(`${location}: integer exceeds safe range`);
    return value;
  }
  const list = /^\[([A-Za-z0-9_, -]*)\]$/.exec(text);
  if (list) return list[1].trim() ? list[1].split(',').map((item) => item.trim()) : [];
  if (!text || /[{}\[\]"'&*!|\x00-\x1f\x7f]|:\s/.test(text)) {
    throw new TypeError(`${location}: unsupported scalar; use a double-quoted string`);
  }
  return text;
}

export function parseCatalogYaml(source, { root, fields }) {
  if (typeof source !== 'string' || Buffer.byteLength(source, 'utf8') > 65_536) {
    throw new TypeError('Catalog must be UTF-8 text of at most 64 KiB');
  }
  const entries = [];
  let foundRoot = false;
  let empty = false;
  let current;
  for (const [index, original] of source.replace(/\r\n/g, '\n').split('\n').entries()) {
    const location = `Catalog line ${index + 1}`;
    if (/[\x00-\x09\x0b-\x1f\x7f]/.test(original)) {
      throw new TypeError(`${location}: control characters are not supported`);
    }
    const line = uncomment(original);
    if (!line) continue;
    if (line === `${root}:` || line === `${root}: []`) {
      if (foundRoot) throw new TypeError(`${location}: duplicate root`);
      foundRoot = true;
      empty = line.endsWith(' []');
      continue;
    }
    const item = /^  - ([a-z_]+): (.+)$/.exec(line);
    const field = item ?? /^    ([a-z_]+): (.+)$/.exec(line);
    if (!foundRoot || empty || !field || !fields.includes(field[1]) || (!item && !current)) {
      throw new TypeError(`${location}: unsupported catalog YAML`);
    }
    if (item) {
      current = {};
      entries.push(current);
    }
    if (Object.hasOwn(current, field[1])) throw new TypeError(`${location}: duplicate field`);
    current[field[1]] = scalar(field[2], location);
  }
  if (!foundRoot || (!empty && !entries.length)) throw new TypeError(`Catalog needs a ${root} list`);
  return entries;
}

export function formatCatalogYaml(root, entries) {
  if (!entries.length) return `${root}: []\n`;
  const scalar = (value) => Array.isArray(value) ? `[${value.join(', ')}]`
    : typeof value === 'string' ? JSON.stringify(value) : String(value);
  return `${root}:\n` + entries.map((entry) => Object.entries(entry).map(([key, value], index) =>
    `${index === 0 ? '  - ' : '    '}${key}: ${scalar(value)}\n`).join('')).join('');
}
