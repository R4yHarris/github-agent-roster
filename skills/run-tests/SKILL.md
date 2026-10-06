---
name: run-tests
description: Verify TASK.md acceptance checks using only Node's built-in node --test runner and report exact results without installing dependencies.
---

# Run tests

## When to use

Use this skill in the assigned worktree after reading its `TASK.md` and
repository instructions. The runner is **`node --test` only**, on Node 20+.
Do not install packages, introduce another runner, or substitute lint, build,
or live network workflows for the acceptance checks.

In the builtin runtime, use the `run_test` tool: it runs the tests that cover
the changed files (including every `tests/<module>.<topic>.test.mjs` shard of a
split module test) and accepts no arbitrary shell commands. The harness runs
the full suite as final verification. The focused command examples below
apply to a manual worker with shell access.

## Stop condition

Stop after the final edit has passing test evidence, or report the first
failure, missing prerequisite, or exhausted turn budget. Never mark unrun
or skipped acceptance checks as passed.

## Procedure

1. Map acceptance IDs to the relevant existing or newly added test files and
   named assertions. Check expected outputs and forbidden side effects, not just
   whether code ran. Report acceptance checks without test coverage so the
   caller can add tests or perform an explicitly planned manual check.
2. From the assigned worktree's repository root, run the smallest invocation
   covering the change. Name test files explicitly and combine related files
   rather than running the same suite repeatedly. Use existing fixtures and
   mocks; do not supply credentials or perform live GitHub actions.
3. If the task requires the full suite, or the change affects behavior beyond
   the focused tests, run `node --test`. Do not claim a focused run covered the
   full suite.
4. Capture the exact command, exit code, test names, and pass/fail/skip counts.
   Confirm the intended tests actually ran. Zero matched tests, skipped tests,
   cancelled tests, and todo tests are not evidence of a passed acceptance check.
5. For a failure, retain the failing test and diagnostic, identify whether the
   cause is the change, a prerequisite, or a pre-existing problem, and return
   that evidence to the implementation step. Rerun after in-scope fixes. Never
   hide failure by skipping tests or relaxing the task's expected behavior.
6. Record evidence for each acceptance ID in the final handoff. The builtin
   runner writes `RESULT.md`; do not edit protected generated task files.
   Missing prerequisites,
   unsupported verification requirements, and unrun checks remain explicit
   blockers; do not fabricate success or install tooling to work around them.

## Commands

These examples use test files already in this repository. Choose files matching
the actual task. Put runner options before file arguments; avoid shell-dependent
wildcards.

```sh
node --test tests/issue.test.mjs
node --test tests/issue.test.mjs tests/recipe.test.mjs
node --test --test-name-pattern="rejects invalid issue numbers" tests/issue.test.mjs
node --test
```

A name-filtered run is useful for diagnosis but proves only matched tests.
Run all cases needed by the acceptance plan, not just the first passing match.

## Result format

```text
Acceptance IDs: <checks covered>
Command: <exact node --test invocation>
Exit code: <actual exit code>
Tests: <passed / failed / skipped / cancelled / todo counts>
Evidence: <test names and the behavior their assertions verified>
Uncovered checks or blockers: <details, or None>
```

Report manual verification separately; this skill does not turn a Node test
result into proof of a manual check. Do not publish, merge, or alter policy as
part of running tests.
