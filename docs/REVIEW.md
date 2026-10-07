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

A completed semantic review that fails is feedback, not a terminal verdict.
`roster run` returns its findings to a fresh coder context (`Review repair N
of 2`) with the unmet check numbers and reasons, keeps worktree edits, then
reviews the repaired result again. If a repair leaves the same checks unmet,
the next round is told to change strategy and, when auto-routing, moves to the
next eligible fleet profile. A reviewer that could not complete (invalid JSON
or endpoint failure), a non-LLM coder, and explicit `--skip-review` do not
trigger repair. After two repairs the last REVIEW.md stands. A later rerun
that reuses the same TASK starts its coder from that failed REVIEW.md's
findings rather than repeating the same attempt. If an interrupted run (for
example, an endpoint failure) archived that review without writing a new one,
the newest archived REVIEW.md is used instead.

The reviewer judges each check by its own words under the TASK.md
Constraints, which bind the reviewer too: it may not add unstated conditions
or require something the task forbids. A re-review after a repair, including
the first review of a rerun that carried findings, receives the previous
findings and judges whether the diff resolves each. A new finding must cite
check or TASK.md wording the change violates, and a check met before stays met
unless the diff regressed it. This keeps repair rounds converging instead of
spending them on a fresh hunt.

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
With `--auto-model`, the reviewer is not the coder (spec 4.8). It routes to the best
eligible fleet profile other than the coder's current profile, and the run log names
both profiles. If that endpoint fails, Roster records the failure as route evidence
and tries the next non-coder profile without moving the coder's route. Only when no
other profile is eligible does the reviewer fall back to the coder's model, and the
log says so. If an independent reviewer returns an incomplete review (for example,
malformed checks after its JSON repair turn), Roster removes that REVIEW.md and
retries once on the next independent profile, excluding the coder's and the
incomplete one; with none left, the incomplete failing review stands. Both
attempts are recorded in run metrics, so the discarded review's tokens still count.
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
as black-box CLI coverage. A source-scan guard that reads repository source
(a relative `src/`, `lib/`, `bin/`, or `scripts/` path) and asserts on it also
counts, because a regression in that source fails it. A seeded string literal that an assertion
checks for absence must reach an app call as an argument, a config object
derived from it, or `process.env`. A diff that changes only test files must
add a test block or assertion when TASK class is `test` or the additions are
only imports and comments; otherwise it is not test work. Failures start with
`Test substance:` and
get one correction turn, shared with the secret-material correction. A
substance failure left after that correction ends the coder context, not the
run: it escalates to a fresh perspective like a stalled repair. A secret or
other gate failure still stops the run. The coder context also lists export signatures of the
modules that allowed files import directly, under Public seams, so the
coder does not spend turns probing for APIs.

`RESULT.md` quotes the verified run's test totals and, for each changed test
file, how many of its declared tests the run reported, with their pass/fail
lines. The reviewer then sees evidence for new tests even when the full-suite
output head does not reach them. `RESULT.md` and `REVIEW.md` are written by the
harness: a coder write to either is denied as a correctable tool error, so a
review repair answers missing-evidence findings with passing tests, not by
editing the report.

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
