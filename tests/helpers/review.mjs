// Builds a passing reviewer reply that judges every numbered acceptance check the harness sent.
export function reviewedChecks(body) {
  const evidence = body?.messages?.find(({ role }) => role === 'user')?.content ?? '';
  const section = evidence.split('## TASK.md acceptance checks')[1]?.split('\n## ')[0] ?? '';
  return [...section.matchAll(/^(\d+)\. /gm)].map(([, id]) => Number(id));
}

export function reviewedFindings(body) {
  const evidence = body?.messages?.find(({ role }) => role === 'user')?.content ?? '';
  const section = evidence.split('## Previous review findings')[1]?.split('\n## ')[0] ?? '';
  return [...section.matchAll(/^(\d+)\. /gm)].map(([, id]) => Number(id));
}

export function passingReview(body, { reasons = [], security_notes = [] } = {}) {
  const findings = reviewedFindings(body);
  return JSON.stringify({ verdict: 'pass', reasons, security_notes,
    ...(findings.length ? { findings: findings.map((id) => ({ id, resolved: true,
      evidence: `Finding ${id} is resolved by the fixture diff.` })) } : {}),
    checks: reviewedChecks(body).map((id) => ({ id, met: true, evidence: `Check ${id} is shown by the diff.` })) });
}
