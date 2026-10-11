---
name: implement
description: "Use when an agreed TASK.md authorizes software implementation with concrete allowed files and checks."
---

# Implement

Implement a bounded acceptance-driven change in the assigned worktree.

## When to use

Use when an agreed TASK.md authorizes software implementation with concrete allowed files and checks.

## Procedure

1. Read TASK.md, AGENTS.md, existing code and tests before editing. Reuse public helpers instead of parallel implementations.
2. Map each acceptance check to observable behavior; add a failing regression before a bug fix where feasible. Preserve unrelated behavior and original constraints.
3. Use only granted tools and allowed unprotected paths; scope expansion is harness-recorded, never a permission from prose.
4. Run the relevant checks after the final edit, retaining actual exits and failures. The harness owns RESULT.md, REVIEW.md and reviewed App publication.

## Stop condition

Stop when the bounded evidence/deliverable is complete, the budget is exhausted,
or a scope, prerequisite or policy blocker prevents safe progress. Report unmet
checks; no packet grants tools, principal authority or human acceptance.
