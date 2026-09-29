---
name: result-report
description: Report factual changes, checks, and the first remaining gap for the harness result.
---

# Result report

## When to use

Use at completion, at a blocking failure, or when the turn budget is exhausted.

## Steps

1. State the task and changed paths, not activity or promises.
2. Report each acceptance check with observed evidence: command, exit code,
   and relevant behavior. Distinguish skipped, unrun, and failed checks.
3. State the first failure and remaining gap plainly. A stub reports that it
   did not implement code or execute tests; it must not invent a diff.
4. Return a concise summary for the harness to write to RESULT.md. Do not
   overwrite that managed file or include credentials, source bodies, or raw
   verbose tool output in the seat notebook.
5. Leave publication to the reviewed App SDK handoff; do not impersonate the
   human, claim an unconfirmed merge, or grant yourself extra capabilities.

## Stop condition

Stop after a truthful, evidence-backed result or explicit failure report.
