// Builds a passing reviewer reply that judges every numbered acceptance check the harness sent.
export function reviewedChecks(body) {
  const evidence = body?.messages?.find(({ role }) => role === 'user')?.content ?? '';
  const section = evidence.split('## TASK.md acceptance checks')[1]?.split('\n## ')[0] ?? '';
  return [...section.matchAll(/^(\d+)\. /gm)].map(([, id]) => Number(id));
}

export function passingReview(body, { reasons = [], security_notes = [] } = {}) {
  return JSON.stringify({ verdict: 'pass', reasons, security_notes,
    checks: reviewedChecks(body).map((id) => ({ id, met: true, evidence: `Check ${id} is shown by the diff.` })) });
}
