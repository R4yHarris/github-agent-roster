function secretValues(env, apiKeyEnv) {
  return Object.entries(env).filter(([name, value]) =>
    typeof value === 'string' && value && (name === apiKeyEnv ||
      /TOKEN|PASSWORD|SECRET|PRIVATE_KEY|API_KEY/i.test(name)))
    .map(([, value]) => value).sort((left, right) => right.length - left.length);
}

const credentialPattern = /\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{24,})\b/g;
// Committed-file scan: a PEM header counts only when base64 key material follows it (raw or in a string literal).
const pemKeyPattern = /-----BEGIN [^-\r\n]*PRIVATE KEY-----(?:\\[rn]|[\s'"`+,])*[A-Za-z0-9+/=]{40,}/g;

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
  let safe = text;
  for (const value of secretValues(env, apiKeyEnv)) safe = safe.split(value).join('[redacted]');
  return safe
    .replace(/-----BEGIN [^-\r\n]*PRIVATE KEY-----[\s\S]*?(?:-----END [^-\r\n]*PRIVATE KEY-----|$)/g,
      '[redacted private key]')
    .replace(credentialPattern, '[redacted credential]');
}
