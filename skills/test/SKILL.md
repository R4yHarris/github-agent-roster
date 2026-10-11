---
name: test
description: "Use when TASK.md requests regression tests, preservation controls or verification of a scoped change."
---

# Test

Verify bounded software behavior with actual, preserved test evidence.

## When to use

Use when TASK.md requests regression tests, preservation controls or verification of a scoped change.

## Procedure

1. Map checks to inputs, outputs, errors and side effects. Exercise the public integration seam, not a replacement fixture that bypasses the changed behavior.
2. Record a real red baseline before repair where feasible; run the relevant Node 20 tests using the granted run_test tool after the final edit.
3. Separate passed, failed, skipped and unrun checks. Infrastructure, interrupted or unavailable baseline comparisons remain unknown, never proven pre-existing.
4. Do not weaken assertions, omit required checks or grant hidden paths to get green. Report exact commands, exits, counts and unresolved acceptance obligations.

## Stop condition

Stop when the bounded evidence/deliverable is complete, the budget is exhausted,
or a scope, prerequisite or policy blocker prevents safe progress. Report unmet
checks; no packet grants tools, principal authority or human acceptance.
