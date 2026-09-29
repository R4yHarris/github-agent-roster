---
name: small-diff
description: Keep a task's implementation complete, focused, and compatible with parallel work.
---

# Small complete diff

## When to use

Use while choosing an implementation and before reporting the final changes.

## Steps

1. Map each edit to TASK.md acceptance checks and allowed paths.
2. Reuse existing helpers, naming, formatting, and dependencies. Avoid unrelated
   refactors, generated files, policy/workflow edits, and vendor changes.
3. Preserve other agents' work. For example, retain existing difficulty and
   estimate fields when adding task skill metadata; do not replace the template.
4. Include tightly related tests and documentation, then verify behavior after
   the last edit. A smaller but incomplete change is not a successful result.

## Stop condition

Stop when all necessary edits are scoped and verified. If completion requires
wider scope or conflicts with another task, report the exact gap without
overwriting other work or weakening acceptance checks.
