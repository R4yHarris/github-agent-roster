function secretValues(env, apiKeyEnv) {
  return Object.entries(env).filter(([name, value]) =>
    typeof value === 'string' && value && (name === apiKeyEnv ||
      /TOKEN|PASSWORD|SECRET|PRIVATE_KEY|API_KEY/i.test(name)))
    .map(([, value]) => value).sort((left, right) => right.length - left.length);
}

const credentialPattern = /\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{24,})\b/g;
// API-key-like assignments, e.g. api_key = "..." or "x-api-key": "..."
const apiKeyPattern = /(?<apiKeyKey>[A-Za-z0-9_-]*(?:api[_-]?key|x-api-key)[A-Za-z0-9_-]*)\s*[:=]\s*(?<apiKeyValue>"[^"]+"|'[^']+'|[^\s,;]+)\b/g;
// Generic secret assignments, e.g. secret = "..." or "secret": "..."
const secretPattern = /(?<secretKey>[A-Za-z0-9_-]*secret[A-Za-z0-9_-]*)\s*[:=]\s*(?<secretValue>"[^"]+"|'[^']+'|[^\s,;]+)\b/g;
// Committed-file scan: a PEM header counts only when base64 key material follows it (raw or in a string literal).
const pemKeyPattern = /-----BEGIN [^-\r\n]*PRIVATE KEY-----(?:\\[rn]|[\s'"`+,])*[A-Za-z0-9+/=]{40,}/g;
// Evidence redaction: a PEM header with its key body (optional RFC 1421 headers, base64 lines) through the footer.
// A header with no key body (prose, a fixture constant) stays, so it cannot erase the rest of a reviewed diff.
const pemSeparator = String.raw`(?:\\[rn]|[\s'"` + '`' + String.raw`+,])*`;
const pemBlockPattern = new RegExp(String.raw`-----BEGIN [^-\r\n]*PRIVATE KEY-----` +
  String.raw`(?:${pemSeparator}[A-Za-z][A-Za-z-]*:[^\r\n\\'"` + '`' + String.raw`]*)*` +
  String.raw`(?:${pemSeparator}[A-Za-z0-9+/=]{16,})+` +
  String.raw`(?:${pemSeparator}[A-Za-z0-9+/=]*${pemSeparator}-----END [^-\r\n]*PRIVATE KEY-----)?`, 'g');

function redactKeyValue(match, key, value) {
  const sentinel = /api[_-]?key|x-api-key/i.test(key) ? '[REDACTED:API_KEY]' : '[REDACTED:SECRET]';
  return `${key}: ${sentinel}`;
}

// 1-based lines holding secret material; prose about PEM envelopes and fixtures without key bodies are not secrets.
export function secretMaterialLines(text, { env = process.env, apiKeyEnv = 'ROSTER_API_KEY' } = {}) {
  const source = String(text);
  const offsets = [];
  for (const value of secretValues(env, apiKeyEnv)) {
    for (let index = source.indexOf(value); index !== -1; index = source.indexOf(value, index + value.length)) offsets.push(index);
  }
  for (const pattern of [credentialPattern, pemKeyPattern]) {
    for (const match of source.matchAll(pattern)) offsets.push(match.index);
  }
  const lines = offsets.map((offset) => source.slice(0, offset).split('\n').length);
  return [...new Set(lines)].sort((left, right) => left - right);
}

export function redactEvidence(text, { env = process.env, apiKeyEnv = 'ROSTER_API_KEY' } = {}) {
  let safe = String(text);
  for (const value of secretValues(env, apiKeyEnv)) safe = safe.split(value).join('[redacted]');
  return safe
    .replace(pemBlockPattern, '[REDACTED:PRIVATE_KEY]')
    .replace(credentialPattern, '[REDACTED:API_KEY]')
    .replace(apiKeyPattern, (match, key, value) => redactKeyValue(match, key, value))
    .replace(secretPattern, (match, key, value) => redactKeyValue(match, key, value));
}

/**
 * Apply redaction recursively to a provenance record (or any plain data tree)
 * before persistence. String leaves are run through `redactEvidence`; objects
 * and arrays are walked recursively. Non-plain values are passed through.
 *
 * @param {unknown} value
 * @param {object} [options] same options as `redactEvidence`
 * @returns {unknown}
 */
export function redactRecord(value, options = {}) {
  if (typeof value === 'string') return redactEvidence(value, options);
  if (Array.isArray(value)) return value.map((item) => redactRecord(item, options));
  if (value && typeof value === 'object' && value.constructor === Object) {
    const redacted = {};
    for (const [key, nested] of Object.entries(value)) {
      redacted[key] = redactRecord(nested, options);
    }
    return redacted;
  }
  return value;
}
