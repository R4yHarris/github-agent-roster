---
name: review
description: "Use when a task requests review preparation or inspection of a scoped diff and its verification evidence."
---

# Review

Inspect a bounded change against unchanged acceptance checks and evidence.

## When to use

Use when a task requests review preparation or inspection of a scoped diff and its verification evidence.

## Procedure

1. Read the original Ask, TASK.md checks, diff and actual test evidence. Keep each unmet obligation visible; model prose or a green badge is not a measured check.
2. Identify concrete logic errors, missing preservation controls and unsupported claims with relevant paths and observable consequences.
3. Inspection is read-only unless a separate coder TASK grants a bounded repair. This packet never turns a coder into the independent reviewer or authorizes a verdict.
4. Report pass/fail/unknown evidence per check and actionable findings. Only the independent reviewer/harness writes protected REVIEW.md; policy, merge and publication authority remain unchanged.

## Stop condition

Stop when the bounded evidence/deliverable is complete, the budget is exhausted,
or a scope, prerequisite or policy blocker prevents safe progress. Report unmet
checks; no packet grants tools, principal authority or human acceptance.
