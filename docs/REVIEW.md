# Builtin reviewer seat

An issue run executes planner, coder, then reviewer **sequentially in the
same process and worktree**. After the coder writes `RESULT.md`, the reviewer
reads the task's acceptance checks, `RESULT.md`, and the actual Git diff,
including the current contents of untracked task files. It writes a root
`REVIEW.md` with a `Verdict: pass` or `Verdict: fail`, reasons, and security
notes. A stub or coder failure produces an explicit failing review; neither
invented source edits nor a model verdict can turn an unverified coder result
into a passing change. The coder's files and `RESULT.md` are retained on
failure.

A coder HTTP timeout is explicitly incomplete work, not a finished stub
review. RESULT.md records `Outcome: timed out (unverified)`, failing checks
and no verified change. REVIEW.md records `Verdict: fail` with a coder HTTP
timeout reason and says review was not completed, even without a reviewer
endpoint. No reviewer model call or apparent approval can override that
timeout. Task/source evidence stays available for the explicit retry.

The [reviewer principal](../principals/reviewer.md) may comment in its
structured report but has **no model-invokable tools**. Its conduct file
cannot grant `write_file`, source editing, publication, merge, deployment,
or a human AI-Eval. The harness alone creates `REVIEW.md`; the reviewer
cannot edit `src/` or any other worktree file. `REVIEW.md` is a managed
artifact excluded from source diffs, coder writes, and publication staging.
The reviewer's comments and security notes are observations, not a
security audit, GitHub approval, App identity, or policy grant. Human review
and the [human retrospective](RETRO.md) remain separate.

Only task-allowed changed paths are inspected. Protected or secret paths are
not read; binary, missing, overlarge, or unavailable diff evidence fails
closed. The context is bounded by `seat.context_chars`, and known credential
values are redacted before a configured reviewer sends the task, result, and
diff to the selected model. A malformed response or tool request produces
`Verdict: fail` with the reason. No endpoint means a deterministic failing
review, not a fabricated approval.

## Publication gate

Roster-managed `roster run --issue N --publish` and REPL `/publish` require
an unchanged passing `REVIEW.md` by default. Roster verifies the report
and the exact `TASK.md` and `RESULT.md` evidence the reviewer read, then
rechecks the coder's original worktree snapshot before staging task-allowed
changes. A failed or changed report blocks publication without deleting
the coder's work. A failed review also suppresses the run's printed
publisher command.
`review.required: false` (legacy alias `reviewer.required`), explicitly saved through
[onboarding](ONBOARDING.md) or private config, makes only that verdict
optional. The reviewer still runs, and issue PR bodies disclose the
configuration bypass. `publish.enabled: false` blocks publication regardless
of a passing review or `--skip-review`.

Use `--skip-review` only for an **explicit bypass**:

```sh
roster run --issue 42 --publish --skip-review
```

In the interactive shell, use `/publish --skip-review` after `/run 42`
(or `/publish "fix: subject" --skip-review` without a run). The reviewer
still runs and writes its verdict; the flag bypasses only that verdict,
not passing coder tests, excellence, model identity, App credentials,
human-owned policy, CI, or repository protections. The generated issue PR
body discloses the bypass. Without an in-session Roster run, `/publish`
has no trusted review to verify and requires the flag while
`review.required` remains true.

A **direct** `agent-pr.mjs` invocation is outside Roster's gate: the
contracts SDK cannot infer an in-process reviewer report and is not
modified here. Review that change as a human before direct publication,
using only the GitHub App and the [GHCP handoff](GHCP.md). Never edit
human-owned policy or workflows to bypass a review failure.

Run the Node tests with `node --test`. Reviewer regressions exercise
read-only model requests, rejected `write_file` calls, fail/pass reports,
evidence freshness, and explicit bypass without granting source writes.
