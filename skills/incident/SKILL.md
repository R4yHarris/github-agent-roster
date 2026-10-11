---
name: incident
description: "Use when TASK.md requests incident triage, regression diagnosis or an explicitly bounded hotfix."
---

# Incident

Investigate a scoped regression or incident without overriding safety gates.

## When to use

Use when TASK.md requests incident triage, regression diagnosis or an explicitly bounded hotfix.

## Procedure

1. Preserve the observed failure, timing, cancellation and current scope. Identify the smallest permitted reproduction and impacted callers.
2. Separate measured baseline failures, change-induced regressions and unknown comparisons. Missing evidence is not proof of an unaffected system.
3. Propose or make only the repair separately authorized by TASK.md. Urgency does not permit production access, deployment, policy edits or wider write scope.
4. Verify the fix and preservation controls with actual checks, then report remaining risks and escalation needs without inventing recovery or human acceptance.

## Stop condition

Stop when the bounded evidence/deliverable is complete, the budget is exhausted,
or a scope, prerequisite or policy blocker prevents safe progress. Report unmet
checks; no packet grants tools, principal authority or human acceptance.
