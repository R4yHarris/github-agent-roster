const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const maximumBytes = 131_072;

function jsonValue(text) {
  try {
    return { value: JSON.parse(text) };
  } catch {
    return { error: 'Planner tool payload or arguments are not valid JSON' };
  }
}

function embeddedJson(text) {
  const start = text.search(/\{|\[\s*[\[{]/);
  if (start < 0) return null;
  const stack = [];
  let quoted = false;
  let escaped = false;
  for (let index = start; index < text.length; index += 1) {
    const character = text[index];
    if (quoted) {
      if (escaped) escaped = false;
      else if (character === '\\') escaped = true;
      else if (character === '"') quoted = false;
      continue;
    }
    if (character === '"') quoted = true;
    else if (character === '{' || character === '[') stack.push(character);
    else if (character === '}' || character === ']') {
      if (stack.pop() !== (character === '}' ? '{' : '[')) return null;
      if (!stack.length) return text.slice(start, index + 1);
    }
  }
  return null;
}

export function parsePlannerToolCalls(message, { turn = 1, usedIds = new Set() } = {}) {
  if (!isObject(message)) return { error: 'Planner returned no assistant message' };
  let payload = message.tool_calls;
  let fromText = false;
  if (payload === undefined || payload === null || Array.isArray(payload) && !payload.length) {
    const text = message.content;
    if (typeof text !== 'string' || !/"(?:tool_calls|function|arguments|path)"\s*:/.test(text)) {
      return { calls: [], fromText: false };
    }
    if (Buffer.byteLength(text, 'utf8') > maximumBytes) return { error: 'Planner tool payload exceeds 128 KiB' };
    const extracted = embeddedJson(text);
    if (!extracted) return { error: 'Planner text contains an incomplete JSON tool payload' };
    const parsed = jsonValue(extracted);
    if (parsed.error) return parsed;
    payload = parsed.value;
    fromText = true;
  } else if (typeof payload === 'string') {
    if (Buffer.byteLength(payload, 'utf8') > maximumBytes) return { error: 'Planner tool payload exceeds 128 KiB' };
    const parsed = jsonValue(payload);
    if (parsed.error) return parsed;
    payload = parsed.value;
  }
  if (isObject(payload) && Object.hasOwn(payload, 'tool_calls')) payload = payload.tool_calls;
  const entries = Array.isArray(payload) ? payload : isObject(payload) ? [payload] : null;
  if (!entries || entries.length > 3) return { error: 'Planner must emit at most three function tool calls' };
  const ids = new Set(usedIds);
  const calls = [];
  for (const [index, entry] of entries.entries()) {
    if (!isObject(entry) || entry.type !== undefined && entry.type !== 'function') {
      return { error: 'Planner tool call must be a function object' };
    }
    const definition = isObject(entry.function) ? entry.function : entry;
    const name = definition.name ?? (fromText && Object.hasOwn(definition, 'path') ? 'write_file' : undefined);
    if (name !== 'write_file') return { error: 'Planner tool is unavailable; only write_file is allowed' };
    let args = definition.arguments ?? (fromText && Object.hasOwn(definition, 'path') ? definition : undefined);
    if (typeof args === 'string') {
      if (Buffer.byteLength(args, 'utf8') > maximumBytes) return { error: 'Planner arguments exceed 128 KiB' };
      const parsed = jsonValue(args);
      if (parsed.error) return parsed;
      args = parsed.value;
    }
    if (!isObject(args) || typeof args.path !== 'string' || typeof args.content !== 'string' ||
        Object.keys(args).some((key) => !['path', 'content'].includes(key))) {
      return { error: 'Planner write_file arguments must be a JSON object with path and content strings' };
    }
    const id = entry.id ?? `planner-${turn}-${index}`;
    if (typeof id !== 'string' || !id || id.length > 128 || /[\x00-\x1f\x7f]/.test(id) || ids.has(id)) {
      return { error: 'Planner tool call ID is invalid or duplicated' };
    }
    ids.add(id);
    calls.push({ id, type: 'function', function: { name, arguments: JSON.stringify(args) }, args });
  }
  return { calls, fromText };
}
