function positiveNumber(value, field) {
  if (!/^[1-9]\d*$/.test(String(value)) || !Number.isSafeInteger(Number(value))) {
    throw new TypeError(`${field} must be a positive safe integer`);
  }
  return Number(value);
}

export function issueMergeMessage(subject, issueNumber) {
  const number = positiveNumber(issueNumber, 'Issue number');
  if (typeof subject !== 'string' || !subject.trim() || subject.includes('\0')) {
    throw new TypeError('Publish subject must be nonempty text');
  }
  const closing = new RegExp(
    `\\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\\s+(?:[A-Za-z0-9-]+/[A-Za-z0-9_.-]+)?#${number}(?!\\d)`,
    'i',
  );
  if (closing.test(subject)) {
    throw new Error(`Issue #${number} must remain open until human AI-Eval; use Refs #${number}, not a closing keyword`);
  }
  return `${subject.trimEnd()}\n\nRefs #${number}`;
}
