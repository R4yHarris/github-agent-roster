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
The Original Ask remains authoritative: planner checks or Design cannot
weaken, contradict, or omit its requested outcomes. Missing requested
deliverables fail with an Ask-citing reason even when the listed checks pass.
Writing future setup/deployment instructions is not executing them; a request
for public-serving instructions is not satisfied by localhost preview alone.
The reviewer must not invent unrelated requirements or silently rewrite checks.

## Trusted host independent-verification protocol

The host API `runReviewer` optionally accepts
`independentVerification: { receipt }`. Supplying the option requires a valid
receipt; an absent option preserves existing behavior. This is a prerequisite
for independent execution under FEATURE_SPEC sections 5.4, 5.5 and 5.6,
not an execution backend or completion of #581.

A trusted host first calls `captureVerificationSubject({ worktree, files })`
from `src/runtime/independent-verification.mjs`, using the exact changed-file
set in `coderResult.excellence.files`. The subject binds raw TASK.md and
RESULT.md bytes plus sorted product paths and hashes, including absent/deleted
files. Paths are exact, relative, unique and guarded; files must be single-link
regular files of at most 64 KiB. At most 64 product paths are supported.
TASK.md includes the authoritative Ask. This is a changed-file binding, not a
hash of every unchanged dependency, Git revision, environment or external input.

The receipt has exactly `schema_version: 1`, `subject_digest`, and
`observations`. Each of 1-64 observations has exactly:

```json
{
  "id": 1,
  "check_id": 1,
  "expected_output": "line 4",
  "actual_output": "line 3",
  "expected_exit": 1,
  "actual_exit": 1,
  "evidence": "Executed the original-Ask multiline diagnostic"
}
```

Observation IDs are unique integers 1-64; check IDs reference the current
numbered review checks. Every check needs at least one observation. Output
strings are bounded to 2000 characters and exits to integers 0-255; evidence
is a trimmed single line of at most 500 characters. Exact output or exit
mismatches force a failing review even when the model passes. This exact
comparison protocol does not handle approximate/nondeterministic results.
Missing, malformed, incomplete or stale receipts fail as incomplete review,
not approval. The subject is rechecked after model review and before passing
publication; changed bytes require fresh verification. REVIEW.md records the
independent status and redacted evidence, not raw observed outputs.

Only the calling trusted host supplies this input. Model response fields,
TASK.md, RESULT.md, receipt files and other worktree content cannot enable
verification or acquire authority. There is no CLI/config toggle, automatic
receipt loading, callback execution, model tool, Docker dependency, image
pull, network access or workflow/policy change in this protocol slice.
Existing explicit `--skip-review` behavior remains unchanged.

The host must own and authorize observation execution, expected results and
file coverage. The protocol cannot prove that a host actually executed a
test, chose sufficient counterexamples, captured every dependency, or told
the truth. It is not a sandbox or human acceptance. An approved enforceable
execution boundary and end-to-end runner adapter remain required before
general independent verification can be offered through ordinary Roster runs.
Do not describe a model-only review or a receipt-free run as independently
executed verification.

### Explicit Docker execution backend

`runDockerVerification` in `src/runtime/verification-docker.mjs` is a trusted
host-only, explicitly opted-in backend for the receipt protocol. It is not
automatically enabled by the CLI, coder or reviewer. Builtin API wiring is
described below; independent generation remains separate #581 work. Docker must already be installed
and its Linux engine available; normal calls never install it or pull images.
The host supplies an immutable locally acquired `sha256:` image ID. A bounded
probe checks that the image runs Linux Node20 before executing product scripts.

The host supplies the complete snapshot file set and an approved plan of
1-16 observations. Each observation names a snapshotted `.js`, `.mjs` or `.cjs`
script, `args`, `check_id`, `expected_stdout`, `expected_stderr`, `expected_exit`
and a short `evidence` label. Expected results are host authority, not product
output or model testimony. All checks must be covered. There is no shell
command string or model-invokable executor. Node receives `--` before the
absolute script path so script arguments cannot become Node options.

Only exact guarded product files are copied into a temporary snapshot.
TASK/RESULT bytes bind the receipt but are not mounted; absent/deleted product
paths stay absent. Do not include credentials or private artifacts in the
approved product set. Source snapshots use existing 64-file/64-KiB bounds;
required unchanged dependencies and verifier scripts must be explicitly
included in the subject set. Keep the review subject set identical.

Each container uses `--pull never`, no external network, read-only root and
product mount, nonroot UID, all capabilities dropped, no-new-privileges,
256-MiB memory/swap bounds, one CPU, 32 PIDs and a 16-MiB non-executable temporary
filesystem. It receives no host credential environment, home, repository Git
storage, Docker socket or raw worktree mount. The Docker client still operates
on the authorized host daemon; these controls do not protect against a
compromised daemon/kernel or prove absolute hostile-code containment.

Each invocation has a unique named container and a maximum 30-second deadline.
Timeout, abort, oversized output, runtime/image failure, subject drift and
cleanup errors stop explicitly without a passing receipt. Named containers
are removed with an independent cleanup deadline even after cancellation;
cleanup failure requires operator inspection, not silent retry. Temporary
snapshot cleanup also fails explicitly. stdout/stderr are compared exactly;
normal application exit codes 0-124 are observable, while Docker infrastructure
exit codes and signal/resource exits fail as infrastructure.

For intentional local validation only, set `ROSTER_VERIFICATION_TEST_IMAGE`
to the explicitly acquired image ID and run the Docker test file. Without
that variable its real-engine test is reported as skipped; mocked controls
still run. This is not a claim of Docker availability on every platform or
autonomous counterexample discovery.

### Builtin run API integration

Trusted callers of `runBuiltinTask`, `runBuiltinIssue` and `runBuiltinAsk` may
now supply `independentVerification: { docker: { enabled: true, image, plan,
dockerPath, timeoutMs, additionalFiles, verifiers } }`. `plan`, `image` and explicit opt-in are required;
the executable and deadline use the backend defaults when omitted.
This API option is not inferred from config, environment, issue text, coder
output or model response. There is no new CLI flag or implicit image acquisition.
The existing `{ receipt }` reviewer option remains supported.

For each eligible completed LLM coder result, the harness executes the approved
plan against the exact current `excellence.files` plus explicitly host-approved
`additionalFiles` subject before model review. Additional paths are subject to
the same private/path/link/size limits, do not grant coder write authority and
remain bound during review and before publication, even when not changed by the
coder. Unlisted dependencies are not automatically included.
The reviewer receives no execution tools. Executed mismatches override a model
pass and supply unmet check IDs to existing bounded repair; the plan executes
again against new bytes after repair or a review retry. Infrastructure or plan
failure writes an incomplete failing review, not model-approved success.
Failed/timed-out/stub coders never execute the Docker backend. Existing
explicit review bypass semantics remain unchanged.

The host must approve script scope, expected outcomes, check coverage and
unchanged dependencies in advance. A script supplied by the coder is not
automatically an independent test simply because Docker executes it.
The host may supply `verifiers: [{ name: 'oracle.mjs', source: '...' }]` separately
from product files and select `script: 'verifier:oracle.mjs'` in a plan entry.
Names are simple `.mjs` filenames; source is bounded to 64 KiB each, at most
16 scripts and 256 KiB combined. These host-only sources are snapshotted into a
separate read-only `/verifier` mount, never written into the coder worktree.
An oracle can import approved dependencies via absolute `/product/...` paths
and assert counterexamples; the coder cannot replace the oracle by writing a
similarly named product file. The plan arguments, expected outputs and verifier
sources are copied before asynchronous execution, so later caller mutation
cannot change the approved execution in progress. Product scripts and host
scripts share the same isolation, resource limits and explicit cleanup.
Read-only mounting prevents replacing the approved source; it is not an
unspoofable oracle against intentionally hostile product code. A directly
imported product shares the verifier's JavaScript process, and container
isolation still depends on the approved Docker daemon and host kernel.

This is protected execution of a trusted-host oracle, not proof that any
model-generated assertion or expected value is correct. The caller must approve
counterexamples from the Original Ask independently; nothing discovers scripts
or grants execution from model testimony. Independent counterexample generation
and user-facing onboarding remain further #581 work. The optional API wiring
does not establish autonomous semantic detection or accept the whole initiative.

For directory-scoped Asks, a separate bounded, read-only review first sees only
the Original Ask and delivered diff, not the planner checklist or coder prose.
It judges whether all requested deliverables are usable and semantically
correct. Its failure overrides a later checklist pass and is recorded as
`Original Ask audit` reasons. Its verdict and evidence are recorded in a
separate report section, including successful audits. Unchanged allowed
deliverables are read into the bounded evidence rather than treated as absent.
Audit and checklist calls use the same configured reviewer model; their returned
usage is combined, with missing counts remaining unknown. Invalid audit output gets one
bounded corrective reply, then fails explicitly as incomplete review, not
approval. This is an independent evidence pass
within the same reviewer seat, not a new worker or authority grant.

For setup-documentation Asks, the audit traces one written path from the
initial artifact through concrete transition steps to the requested observable
end state. A separate bounded call focuses only on the transition from the
stated prerequisites into the usable destination, without the broad verdict or
planner checklist. It is recorded as the second numbered audit check. Its
unmet verdict overrides a summary pass, using the existing per-check gate.
Provider names or DNS/TLS terminology alone do not prove the missing upload/publishing procedure.
One usable dashboard or command path suffices; multiple providers and
deployment execution are not required. A prepared page may be a prerequisite
when the Ask requests design and instructions rather than implementation.
Passing transition evidence must quote an operational step from the delivered
evidence, rather than paraphrase a recommendation into a command that was
never written. The harness checks that excerpt after redaction, whitespace
normalization, and removal of Markdown backticks/bold markers; an invented
excerpt gets the same bounded correction and explicit incomplete-review failure
as other invalid audit output. This
source check is not a keyword-based proof of procedural completeness or a
replacement for semantic review and human acceptance.
The focused response also extracts the operation kind: an explicit tool
invocation, a named UI/upload control, or a missing mechanism. An invocation/subcommand or
UI control absent from the cited step, or an explicitly missing
mechanism fails the numbered check even when the model's summary says pass.
Full allowed artifacts are read through the existing guarded text reader for
this extraction, so a small diff does not hide operational steps or prerequisites.
This adds one reviewer call for setup/installation/deployment/hosting
documentation, plus at most one corrective reply for invalid output; all
calls remain within the same seat, task deadline, and per-context budget.

Separately from checks, the reviewer probes the changed code for five defect
classes: destructive or permissive defaults for missing input, data that
bypasses redaction, identifiers that collide for distinct records, and
resources not released on every path or releasable by the wrong holder, and
incorrect observable results contrary to the requested operation or an
established mathematical invariant (even when generated tests expect them). It
reports each as an optional `defects` entry `{file, symbol, input, outcome}`
naming a changed file. Any defect turns the verdict to `fail` with a
`Defect in <file> <symbol>: <input> -> <outcome>` reason, even when every check
is met, so review repair sends it back to the coder. Checks never permit
defects; style and unrequested features are not defects.
The evidence also lists `## Unchanged existing files`: allowed files and
relative imports of changed files that exist at the base but are not in the
diff, so an unchanged module is not judged missing.

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
Constraints when consistent with the Original Ask: it may not add unrelated
conditions or execute forbidden actions. A planner constraint cannot omit
requested documentation. Original Ask authority is supplied separately from
the checklist so a matching checklist cannot override the requested outcome.
A re-review after a repair, including
the first review of a rerun that carried findings, receives the previous
findings numbered from 1 and must return `findings`: exactly one
`{id, resolved, evidence}` judgment per finding. IDs must be unique and in range;
`resolved` is boolean and evidence is a trimmed single line of at most 500
characters citing current code or verification. Missing or invalid accounting
uses the existing one JSON correction, then leaves an incomplete failing review.
Any `resolved: false` overrides a checklist pass; `REVIEW.md` records the
judgments under `## Previous findings`. Prior evidence is never silently
clipped to fit context; overflow fails explicitly. A green test that does not
exercise the reported failure is not resolution evidence. These judgments
remain model claims, not independent proof or human acceptance.
With no prior findings, the existing JSON shape is unchanged. A new finding must cite
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
diff to the selected model. Redaction keeps source parseable: unquoted code
references such as `api_key: SENTINEL` or `options.apiKey` stay, quoted
values keep their quotes, and the reviewer is told that redaction markers are
evidence substitutions, not code. A marker in a test input means a secret-like
value, usually the test-only sentinel, was present. A malformed response or tool request produces
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
test are failed as tautological. A test title is not evidence: each named case
in a check needs its own assertion on app output, and persistence or recovery
tests must read the result back through the app. `assert.ok` on a returned
object, or an assertion on the test's own clock or inputs, does not count.

Before review, the coder's excellence gate applies the same rule
deterministically to added lines of changed test files, regardless of model.
A new test block must call an imported app function, directly or through a
file-local helper, whenever the file imports one; `child_process` spawns count
as black-box CLI coverage. A source-scan guard that reads repository source
(a relative `src/`, `lib/`, `bin/`, or `scripts/` path) and asserts on it also
counts, because a regression in that source fails it, and so does asserting on
an imported SCREAMING_CASE app constant. Calling a method on an object an app
function returned (such as `store.query()`) is an app call; a built-in method
on it, such as `includes`, is not. A seeded string literal that an assertion
checks for absence must reach an app call as an argument, a config object
derived from it, or `process.env`; a template interpolation such as
`` `key ${SENTINEL}` `` inside the call counts as passing it. Changed app code must not
contain a sentinel that a changed test seeds; special-casing the test value is
gaming, and the reviewer fails it too. A diff that changes only test files must
add a test block or assertion when TASK class is `test` or the additions are
only imports and comments; otherwise it is not test work. Failures start with
`Test substance:` and
get one correction turn, shared with the secret-material correction. A
substance failure left after that correction ends the coder context, not the
run: it escalates to a fresh perspective like a stalled repair. A secret or
other gate failure still stops the run. The coder context also lists export signatures of the
modules that allowed files import directly, under Public seams, so the
coder does not spend turns probing for APIs. On Windows, a slice with test
files also gets a Test host note: file-symlink tests skip without privilege and
a skipped test is not evidence, so link escapes are tested with directory
junctions.

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
