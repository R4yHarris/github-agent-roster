export function redactEvidence(text, { env = process.env, apiKeyEnv = 'ROSTER_API_KEY' } = {}) {
  let safe = text;
  const values = Object.entries(env).filter(([name, value]) =>
    typeof value === 'string' && value && (name === apiKeyEnv ||
      /TOKEN|PASSWORD|SECRET|PRIVATE_KEY|API_KEY/i.test(name)))
    .map(([, value]) => value).sort((left, right) => right.length - left.length);
  for (const value of values) safe = safe.split(value).join('[redacted]');
  return safe
    .replace(/-----BEGIN [^-\r\n]*PRIVATE KEY-----[\s\S]*?(?:-----END [^-\r\n]*PRIVATE KEY-----|$)/g,
      '[redacted private key]')
    .replace(/\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{24,})\b/g,
      '[redacted credential]');
}
