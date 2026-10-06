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

The verdict is derived from per-check evidence. The reviewer receives the
acceptance checks numbered and must return `checks`: one
`{id, met, evidence}` entry per check, citing the diff file and symbol or the
RESULT.md output. A `pass` that omits any check gets one corrective retry,
then fails. Any `met: false` turns the verdict to `fail` with
`Check N unmet: …` reasons, whatever the model's summary says. REVIEW.md lists
each judged check under `## Acceptance checks`.

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
For test tasks and changed test files, the reviewer is additionally instructed
to verify that assertions fail when the requested behavior is absent, and to
exercise the public operation when the Ask names one. Secret-leakage checks
must feed an obvious non-credential sentinel such as `test-only-private-api-key`
into the app code under test and assert that exact value is absent from its
serialized output; a generic keyword scan, or a sentinel the test strips
itself, is not sufficient. Tests that only inspect objects built inside the
test are failed as tautological.

Before review, the coder's excellence gate applies the same rule
deterministically to added lines of changed test files, regardless of model.
A new test block must call an imported app function, directly or through a
file-local helper, whenever the file imports one; `child_process` spawns count
as black-box CLI coverage. A seeded string literal that an assertion
checks for absence must reach an app call as an argument, a config object
derived from it, or `process.env`. A diff that changes only test files must
add a test block or assertion when TASK class is `test` or the additions are
only imports and comments; otherwise it is not test work. Failures start with
`Test substance:` and
get one correction turn, shared with the secret-material correction; a second
failure stops the run. The coder context also lists export signatures of the
modules that allowed files import directly, under Public seams, so the
coder does not spend turns probing for APIs.

## Publication gate

Roster-managed `roster run --issue N --publish` and REPL `/publish` require
an unchanged passing `REVIEW.md` by default. Roster verifies the report
and the exact `TASK.md` and `RESULT.md` evidence the reviewer read, then
rechecks the coder's original worktree snapshot before staging task-allowed
changes. A failed or changed report blocks publication without deleting
the coder's work. A failed review also suppresses the run's printed
publisher command.
When the review gate is explicitly disabled or `--skip-review` is supplied,
the printed command includes a warning that the failed review is being
bypassed, not approved. The shell shows the review failure reason instead
of its normal reviewed-publication hint. These settings never bypass
operational excellence: a bounded code/test task that produces no application
diff fails before reviewer inference and cannot be published as an implementation.
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
