# Operational excellence for one task

The [excellence gate](../src/runtime/excellence.mjs) runs after the coder's tool
loop and memory append, before the harness creates RESULT.md. It does not
grade its own work with a human AI-Eval or claim that a passing test proves
every requirement.
For a configured coder, the same gate also checks each final summary
inside the bounded tool loop. Passing final tests and a passing gate end
the loop; a failed final test can be repaired within the remaining
turn budget using redacted diagnostics. Protected-path and secret
failures stop immediately rather than offering a chance to conceal them.
After memory is recorded, the gate runs again against the verified
worktree snapshot before writing RESULT.md. The empty-URL stub has no
preliminary pass and remains an explicitly unverified demonstration.

Its checklist is:

- Tests were executed successfully after the final edit, unless initial task
  frontmatter explicitly declares `tests: none`.
- Changed paths stay inside TASK.md's allowed paths and outside protected
  policy, workflow, secret, contracts, evaluation, notebook, and managed paths.
- Changed file bodies and the Git diff contain no detected secret material.
- The result has a summary and records the actual model ID and tool-loop turns.
- RESULT.md begins with checks passed or the first failure, not an unsupported
  success claim.

## Evidence, not tool claims

The harness snapshots the worktree before the tool loop and compares it
afterwards, so writes by test subprocesses are checked too. It also checks the
actual tracked/untracked Git diff, including changes that predate the turn.
For an offline non-Git fixture it uses the before/after snapshot. Protected
file bodies are never opened: metadata changes are enough to fail their gate.
Symlinks and special files cannot pass as regular changed source files.
The harness's own RESULT.md and append-only notebook are not app-code diffs.

Secret detection recognizes known configured/environment credential values,
private-key headers, and common GitHub/API token forms. Diagnostics identify
the path, not the secret; result evidence is redacted. This is a conservative
check, not proof that arbitrary text is free of secrets or an OS sandbox.
See the [threat model](THREAT_MODEL.md).

## Failure and publication

`checkExcellence` returns `{ pass, reasons, files, model, turns, snapshot }`;
the snapshot is private in-process verification evidence, not file bodies.
Failures,
including final test failures and turn-budget exhaustion, still produce
RESULT.md and explicit error results. A configured coder rejects after writing
that report. If a gate discovers a new failure, the notebook receives a second
append-only failure entry rather than rewriting the first entry.
The run journal retains each redacted failure reason in `defects`. Paired
issue runs record these automatically; standalone coder runs use their
existing opt-in journal. New publication-time gate failures append another
run record rather than erasing the original verification.

The stub remains a successful *offline demonstration*, not completed software:
it makes no code diff, runs no tests, and writes a failed/unverified excellence
report without throwing solely because its acceptance checks were not run.
It cannot publish. A task's `tests: none` waives automatic test execution, not
scope, secrets, principal restrictions, or truthful reporting.

Publication requires a passing gate and rechecks it immediately before staging,
including equality with the verified snapshot, so an edited file cannot reuse
an earlier passing test.
No publication happens automatically on excellence failure. Reviewed publication
still requires the explicit [App SDK handoff](GHCP.md).
Recorded defects count as rejects in recommendations, including secret-path
and policy touches that are later accepted by a human. Passing checks do not
create an acceptance: the human must still run `roster eval`.

The model and usage feed the existing contracts AI-Run. Tool-loop and research
turn counts are recorded alongside it in RESULT.md; the pinned contracts schema
is not extended or rewritten.
