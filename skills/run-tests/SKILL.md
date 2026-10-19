---
name: run-tests
description: "Use when verifying TASK.md acceptance checks with the task language's approved native runner and recording actual evidence."
---

# Run tests

## When to use

Use in the assigned worktree after reading TASK.md and repository instructions.
Roster is Rust: its own changes require Cargo checks, not a Node Roster suite.
JavaScript consumer tasks may use approved Node 20 checks; that does not verify
Rust changes or authorize arbitrary commands, network access or publication.

## Procedure

1. Map each acceptance ID to named assertions, expected outputs and forbidden
   side effects. Port prior cases from Git history without weakening them.
2. Run the smallest approved invocation covering the change. Combine related
   selectors, use existing fixtures and do not contact live services.
3. For Roster, run formatting, strict all-target Clippy and relevant Cargo
   unit/integration tests with locked/offline dependencies. Run the full native
   suite when required; do not claim a focused run covered it.
4. Record command, compiler/platform, exit code, actual tests and pass/fail/
   skip/cancel counts. Missing matches, skips and unavailable checks are not
   passes. A passing Node consumer test is not native verification.
5. Retain failures and diagnostics, repair the cause within scope and rerun.
   Never relax an assertion, skip a failing case or call removed coverage green.
6. Report uncovered checks and platform limitations. Harness-written RESULT.md
   remains protected; no publishing, policy or workflow authority is granted.

## Stop condition

Stop after the final edits have actual passing evidence, or report the first
unresolved failure, missing prerequisite or exhausted budget. Removal of the
old implementation does not prove the replacement complete.
