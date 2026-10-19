# Testing

This migration branch uses native Rust unit and integration tests. The Node
application, Node suite and differential-oracle infrastructure have been
removed at Ray's direction. Main/Git history preserves the original acceptance
cases for translation, not a second implementation or checkout on this branch.

## Commands

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --all-targets --locked --offline
cargo test --test cli --locked --offline
cargo test --release --all-targets --locked --offline
```

Run the smallest selector covering the change, then the required broader gates.
Use an already installed Rust 1.85 toolchain to check the minimum compiler:

```powershell
cargo +1.85.0 fmt --all -- --check
cargo +1.85.0 clippy --all-targets --locked --offline -- -D warnings
cargo +1.85.0 test --all-targets --locked --offline
```

Ordinary checks do not install dependencies or toolchains. Restore missing
dependencies only after the manifest/lock is defined or a missing-dependency
failure. Keep build output outside the source tree or in ignored `target`.

## External consumer fixtures

Some native process, tool and execution tests operate owned JavaScript consumer
projects. Set `ROSTER_TEST_NODE` to an existing Node 20 executable for those
tests. These fixtures do not import a Node Roster implementation. Tests also
exercise approved Cargo subprocesses with locked/offline flags.

On Windows, `tests/execution_backends.rs` uses real owned Git repositories,
Node consumer/human-hook processes, Cargo integration tests and the compiled
native `roster` executable. It covers base/candidate named red/green,
compiler/setup negatives, inline-Rust unavailability, actual
preexisting/transient/regression classifications, explicit local/nested
submodules, stale receipts, committed hooks and pre-TASK leases, isolated
read-only CLI acceptance and missing/invalid-option controls. A package-name
fixture checks that retired Node Roster production is never materialized.
No Node Roster import, legacy checkout, model, publishing or live service is
needed.

Focused execution-review controls first reproduced all four actual failures:
scoped green hiding a failed full run, partial same-file failure mapping
granting an exception, native recipe reads through a dash-shaped operand, and
`roster.exe` obligations becoming inapplicable. The unchanged earlier native
cases remain alongside new controls for unresolved/regressed/unclassified
full runs, source and observation invalidation, actual full recovery,
preexisting/transient and fully mapped nested positives, duplicate/unknown/
setup/worker failures, mixed Cargo/Node coverage, protected/outside recipe
operands and native executable aliases.

A dependency-free owned Rust read-probe fixture records any invocation, proving
unsafe recipe operands and unavailable/malformed commands do not execute the
CLI. Positive acceptance still runs the real compiled `roster` binary. The
probe is not test-pass authority or a framework implementation. A Node
zero-selected-test startup control is likewise only an execution-hygiene
negative control; skips and zero matches never mint passing test evidence.
Windows worker termination tests are not POSIX signal/containment proof.

Archive/restore tests use real owned Git objects, generated-artifact archives,
tracked/untracked source rewinds and strict issue branches. They cover unknown/
published/unconfirmed approval, changed source/artifacts/refs/index or writer,
protected/secret/link trees, pending-call and old-test revocation, fresh Cargo
checks and retained failed-full obligations. Actual Windows sharing-denied
handles and cancellation barriers exercise rollback and quarantine; a foreign
human task is preserved while only owned source changes are rolled back.
Current-publication sequence fixtures also refuse published/unknown/unavailable
lookups before staging or mutation and roll back a publication change at commit;
they are not live GitHub publication checks.
Boundary close/reopen tests retain failed-full obligations and require new Cargo
checks after a committed restore. Controlled removal of a real completion marker
proves unreconciled-journal refusal; it is not a claim that every hard-kill/crash
position has been exercised.

Focused recovery-review controls first reproduced actual native private-target
installation and committed restoration with late human RESULT/REVIEW evidence.
Consistent owned Git trees, refs and checkpoint metadata exercise absent and
present private paths, case aliases, shared coder/restore denial and legitimate
product restores. All non-preserved artifact absences are checked at confirm
and direct archive execution. Publication callbacks and a biased native async
product-loop barrier add real human files after archival or source mutation;
controls require owned rollback, preservation of new bytes, revocation of old
context/calls/tests and retained failed-full history. Replacement of an
already archived artifact must preserve both versions and quarantine; a new
previously absent artifact must not be deleted or archived. Directory/hard-link
arrivals and unchanged preserved authority have separate controls. These are
Windows filesystem/owned-process proofs, not Unix or live Docker evidence.
The product-loop observation clock starts at the actual mutation-ready
publication barrier, separately from bounded backup/staging readiness; the
fixture still requires arrival before the next product changes and before
the commit publication lookup. Full-suite load does not relax production
deadlines or these ordering assertions.

Docker tests compile a small dependency-free owned Rust CLI fixture with the
selected installed Cargo compiler and strict Clippy, then execute it through
the real native process/receipt/cleanup pipeline. The fixture records arguments
and simulates local image/container metadata, not a real daemon. Controls cover
required opt-in, immutable image and readonly input binding, exact isolation/
namespace/resource settings, output/exit mismatch, unknown CID/labels/absence,
cleanup failure, deadline/overflow/cancellation, source mutation, frozen plans
and unchanged RESULT-subject writing. No pull/build/login, live daemon, image
acquisition, model, network or hardware workload is used. Passing fixture tests
is explicitly **not physical Docker/Linux containment proof**.

Set `ROSTER_TEST_NODE` to an absolute installed Node **20** executable.
`ROSTER_TEST_CARGO_TOOLCHAIN`, when set, selects an already installed approved
toolchain for the owned Cargo consumer fixtures; it does not select the
compiler building Roster. Report both selections. For a fully MSRV-compiled
fixture pass, select the installed 1.85 GNU toolchain there as well as in the
outer Cargo command. Do not install a toolchain or acquire packages as a
side effect of an ordinary test.

## Native analytics controls

Use the smallest analytics selectors; do not interpret a targeted pass as
the entire migration or the pre-existing 642-test baseline being green.
The previous default local-delivery result remains **93/100**, with seven
existing 600-second guard failures. Analytics does not change those guards,
test assertions, resource policies or the owned-process runtime and does not
blindly rerun that default 100/full campaign.

```powershell
$env:ROSTER_TEST_GIT = (Get-Command git).Source
cargo test --lib --locked --offline learning::
cargo test --lib --locked --offline metrics::
cargo test --test analytics --locked --offline -- --nocapture
cargo test --test cli --locked --offline
cargo fmt --all -- --check
cargo clippy --all-targets --locked --offline -- -D warnings
```

Repeat the targeted commands with an installed Rust 1.85 compiler and
`ROSTER_TEST_CARGO_TOOLCHAIN` selecting that same compiler. The read-only
negative probe is an owned dependency-free Rust consumer, compiled offline
with that selection; successful acceptance always executes the actual
compiled `roster`, including its unknown-option control. No mocked passing
JSON, Node oracle, live endpoint, GitHub query or publication is used.

The native checks port the independent `learn`, `metrics` and route-evidence
expectations from Git history: distinct SHA/session samples and corrections,
sticky defects, known-only medians, unknown versus zero, MAX_SAFE_INTEGER and
large decimal counts, real SHA-1/SHA-256 objects/trailers, requested capacity,
per-seat/model ceiling streaks, timestamps and deterministic ties. They also
cover exact byte/line/row/entry/output bounds, secret-free errors, explicit
ref/evaluation precedence, canonical linked-worktree state, foreign identity
denial, corruption preservation and real sharing/hard-link failures.

Every new CLI fixture injects dedicated HOME, USERPROFILE, installation and
ROSTER_STATE_HOME **before invocation**, including missing/invalid cases.
Readonly commands are checked against complete owned file/directory snapshots.
No real operator history, vault, configuration, credentials or other
repository's record is inspected. Seat acceptance refuses external/private,
dash-shaped and linked evaluation operands before any CLI execution, while
valid TASK-scoped inputs run the real binary.

New fixtures are exact `.roster/analytics-tests-<pid>-<nonce>` owners, never a
system temporary directory. Successful cases explicitly remove their owner;
failed cases retain the exact root and print it for hashing/archival before
repair. Do not sweep `.roster`, old forensic roots or sibling synthetic homes.
Windows GNU evidence is not MSVC, POSIX or independent-review acceptance.

The analytics checkpoint passes **56 new tests** (31 learning, nine metrics
and 16 native CLI/e2e) plus **20 unchanged affected controls** on both stable
GNU Rust 1.99.1 and GNU Rust 1.85.0. Both compilers pass format checking and
strict all-target Clippy. Discovery is **698**, not a full-suite pass.
Stable release runs pass all 16 analytics and six unchanged CLI cases.
The preserved controls include three run-metadata, three provenance, six
lossless-planning, six CLI and two guidance cases. The unchanged recipe-file
CLI case creates and removes its own nonce-named temporary root; no fixture
or assertion was rewritten for these results. New analytics fixtures stay
under their recorded repository-owned roots.

Independent source review found four analytics issues. Bounded native
counterexamples reproduced all four before fixes: joined metadata fan-out,
lost human-decision ordering, a SHA correction erasing another commit's
session evaluation, and private paths in recommendation configuration errors.
The repaired controls cover an exact 16 MiB joined snapshot and one-byte
overflow for both metadata and evaluation expansion, existing 32 MiB combined
inputs that deduplicate safely, absent/partial/tied timestamps, independent
SHA/session corrections, ordinal validation and unchanged owned configuration
on text/JSON errors. No multi-gigabyte exhaustion probe was run.
No default local-delivery 100-case or full 698-case execution was used to
overwrite the known blocked result.

## Native human evaluation controls

This implements FEATURE_SPEC **5.6**, **5.7** and **5.8** without permitting
agent-generated human acceptance. Remote human feedback is explicit:

```powershell
$env:ROSTER_TEST_GIT = (Get-Command git -CommandType Application).Source
cargo test --lib --locked --offline evaluation::tests::
cargo test --test analytics --locked --offline human_eval_ -- --test-threads=1
cargo test --test analytics --locked --offline human_post_ -- --test-threads=1
```

The owned fixtures cover full SHA1/SHA256, real abbreviation resolution,
reported model attribution, zero/unknown minutes, missing final newlines,
append-only corrections and deduplication, canonical state scope, redacted
comments, unchanged malformed/hardlinked ledgers, serialized writers, exact
16 MiB/+1-byte and 10,000/+1-row limits. Explicit durable lookup covers
same-identity source references, operational attempt lineage, ambiguous
attempts, missing served models and local evidence preceding a poisoned
machine store. No actual operator history, vault or human evaluation is read
or written by these tests.

Independent review found conflicting local-session model attribution. Its
native CLI counterexample and a coupled private-model URL counterexample both
failed before fixes. The shared ambiguity helper now runs before session
selection, while explicit SHAs retain their exact model. Prior human model
metadata uses the same privacy guard as other sources.

The read-only mutation denial checks the actual `Report::passed() == false`,
`unavailable` status, no executed runs, a specific denial and an absent
native-probe marker. `Ok(Report)` alone is not passing authority. Its initial
new test incorrectly expected an error variant; only that test was corrected
to the existing report contract, with no production admission change.

The local-only checkpoint's affected matrix passed **91/91** on stable GNU Rust 1.99.1 and
GNU Rust 1.85.0, with strict all-target Clippy and format checks. Thirty
unchanged owned-process/timing controls also passed on each compiler. Its
optimized validation passed **27 analytics/feedback integrations + 6 CLI**.
At that checkpoint both compilers discovered **713** tests, not an executed suite.
The default local-delivery campaign remains independently blocked.

The posting fixtures compile a dependency-free owned Rust GitHub CLI and
isolate its config, home and fake token. They do not call GitHub or load real
human credentials. Controls verify exact compact posted bodies, zero minutes,
private-comment exclusion, explicit credential selection, actual-principal
rejection, ambiguous/foreign/malformed associations and no-SHA/no-PR outcomes.
Errors after a local save retain the row, state uncertainty and never retry;
origin drift makes no POST. Real submission barriers cover cooperative
cancellation/owned cleanup and immediate future Drop retaining writer
authority. The original read-probe fixture still verifies execution denial
and now shares a compiler helper that checks the actual compiler exit.

The completed explicit-post checkpoint passed **101 affected tests** on each
compiler, strict all-target Clippy and format checks. Stable optimized runs
passed **33 analytics/feedback integrations + 6 CLI**. Both compilers discovered
**723 tests**; this was not a full-suite run. Independent review found that a
metadata/origin Git cleanup failure could release the evaluation writer despite
unknown cleanup. The failing native counterexample uses a real physical lock
and a trusted cleanup-error input, not a manufactured kernel failure. Final
recording now retains ownership for either provider uncertainty or
`ROSTER_PROCESS_CLEANUP`, including Git failures outside the GitHub client.
The paired known-settled failure still releases its marker. Re-review cleared
the repair; no real human credential, operator evaluation or GitHub write was
used.

The isolated unchanged failed-full workflow also passes **1/1 in 189.45s**.
Two additional passive intervals show 316 first-layer owned job creations taking
**9,667 us**, versus **81,871,297 us** in the wrapped spawn calls. The locked
process wrapper's resume implementation enumerates a global Windows thread
snapshot. This is a concrete candidate, not proof of intrinsic OS, scheduler
or security-software cause. No unsafe resume path, MSRV increase, containment
skip, image/thread/deadline change or default 100-case replay follows it.

The strict owned-process unit controls use actual Windows Jobs and detached
descendants. Cancellation waits for an observed live descendant readiness
barrier; success, timeout and overflow check real descendant liveness after
completion. They do not count a wrapper's root-only wait as cleanup proof.
Unverified cleanup must retain the writer and scratch, not yield a pass.
Unix strict execution is explicitly unavailable until its own provider and
real platform controls are implemented; Windows tests are not Unix evidence.

Fixtures live under the project or their owned repository's Git common
directory and clean only their own containers. Keep the ordinary system
`TEMP`/`TMP`; isolated child homes/configuration/temp directories are part of
the approved backend contract, not a global environment change. No test
fetches submodule URLs or implicitly acquires images/dependencies.

The native CLI tests run with an empty `PATH`, exercise real files and stdin,
compare exact stdout/stderr and exits, test malformed input, and measure the
exact 4 MiB stdin boundary. No model, GitHub or Docker service is required.

## Native offline status controls

Product trace: FEATURE_SPEC **5.7** and **5.8**. The smallest selectors are:

```powershell
$env:ROSTER_TEST_GIT = (Get-Command git -CommandType Application).Source
cargo test --lib --test analytics --locked --offline -- status::tests:: offline_status_cli_ analytics_read_only_acceptance_runs_the_actual_compiled_cli_and_invalid_option_controls --test-threads=1
cargo test --test execution_backends --locked --offline read_only_selection_maps_legacy_spelling_to_native_intent_without_a_source_fallback
```

The first stable GNU run passed six native units and six real CLI/acceptance
cases. They cover required offline/issue flags, safe numbers, cancellation
before discovery, missing observations, safe complete-record projection,
partial UTF-8 tails, canonical/subdirectory/linked-worktree selection, corrupt
complete records, hard links, directories, actual Windows sharing denial and
unchanged complete owned file/directory snapshots. Poisoned endpoint config,
GitHub-image selection and App inputs do not affect a successful status read.
Exact 2 MiB records remain readable; one extra byte fails, while a valid
10,001-record native log remains readable. Large detail bodies never appear
in output. Unknown-option controls invoke the actual CLI.

Read-only acceptance now runs status with automatic `--offline`, alongside the
unchanged stats/recommendation obligations and their negative controls.
Selection checks retain legacy-spelling mapping, unsupported bare-status
refusal and the 32-argument cap after normalization. These controls do not prove
online status, retry selection, artifact/gate display, interactive rails,
cross-platform containment or full migration acceptance.

The affected stable GNU 1.99.1 and GNU 1.85.0 checks pass **113 cases per
compiler**: 66 library cases, 38 analytics/feedback/status integrations, six CLI
cases, two guidance cases and one preserved selection control. Both compilers
pass format checking and strict all-target Clippy and discover **734 tests**,
not 734 executed passes. Rust 1.85 Clippy caught an allocation-heavy iterator
in the new record-count fixture; it was changed to direct checked writes into
one string, without suppressing the lint or changing production behavior.
Stable's six status units were replayed after this test-only repair.

Optimized stable checks pass **51 cases**: six status units, all 38 integrations,
all six CLI cases and the selection control. An initial overbroad release
filter also selected an unchanged JavaScript-consumer hook control without its
required explicit `ROSTER_TEST_NODE`; that setup failure is preserved. Precise
selectors pass, and the exact hook control also passes with its existing
installed Node 20 input. No retired Roster implementation, new dependency,
live model, actual GitHub query or default/full-suite retry is involved.
The existing default local-delivery result remains **93/100**.

## Native App issue observation controls

Product trace: FEATURE_SPEC **5.1**, **5.7** and **5.8**. Use explicit installed
Git and Node20 images through `ROSTER_TEST_GIT` and `ROSTER_TEST_NODE`, then:

```powershell
cargo test --lib --test analytics --locked --offline -- github::tests:: status::tests:: offline_status_cli_ online_status_cli_ analytics_read_only_acceptance_runs_the_actual_compiled_cli_and_invalid_option_controls --test-threads=1
```

Owned fixtures create isolated consumer/installation repositories and a pinned
synthetic SDK interface. They execute the actual owned Node20 adapter with a
dummy JSON key, not a real PEM, App credential or copied SDK source. The mock
native HTTP transport verifies the exact App token/header, fixed GitHub scope,
GET and revocation sequence; it cannot contact GitHub. No live signing or App
installation permission is claimed by this interface simulation.

Controls cover missing credentials before repository access, no ambient human
auth or SDK-CLI auto-execution, exact issue/URL/state shape, PR refusal, secret
exclusion, the exact 1 MiB response bound, mandatory empty revocation
acknowledgement, failures without metadata, origin drift and first-reason
cancellation at actual SDK/HTTP readiness barriers. Returned context rechecks
the original repository and reviewed dependency after later awaits.
Real CLI negative controls preserve owned files and refuse missing App
credentials before local/private providers; existing offline CLI/e2e controls
continue to execute the real binary without auth or networking.

A native counterexample reproduced App key growth after the preflight size
check: the initial SDK-side unbounded read accepted it. The adapter now uses a
bounded regular single-link handle read and clears its buffer. Both the
counterexample and exact 65,536-byte/+1-byte controls pass on stable GNU;
metadata-only size checks are not accepted as actual-read proof.
Successful reads require revocation acknowledgement; a dropped future is not
proof of remote revocation or writer cleanup and returns no issue authority.
The default local-delivery, vault identity, owner CI and native platform/live
acceptance gates remain separate and open.

Independent review found two App-observation bugs. Owned native counterexamples
failed before both repairs: a key inside a linked consumer source tree was
accepted, and cancellation during successful token cleanup lost to the earlier
HTTP failure. Key exclusion now checks current and canonical roots for both the
consumer and installation, including linked-installation fallback. After any
acknowledged revocation, cancellation is rechecked before propagating the
earlier result, with that provider failure retained as a cause. Both new controls
pass on stable GNU and Rust 1.85; the shared Tokio skill captures the
cleanup/error ordering. Re-review cleared both repairs.

The repaired source passes **44 targeted checks per compiler** on stable GNU
1.99.1 and GNU 1.85.0: 28 library cases, seven actual CLI/e2e cases, six CLI,
two guidance and one selection control. Format checking and strict all-target
Clippy pass on both. Actual discovery is **766 tests**, not 766 executed passes.
Before these two review repairs the 42-case affected matrix also passed in
optimized stable mode. Both newly added review controls also pass in optimized
stable mode after the repairs. All App validation remains synthetic/offline; the
default delivery and platform/live acceptance gates have not changed.

## Native offline fleet listing controls

Product trace: FEATURE_SPEC **5.2** and **5.7**. The focused selectors are:

```powershell
$env:ROSTER_TEST_GIT = (Get-Command git -CommandType Application).Source
cargo test --lib --test analytics --locked --offline -- fleet::tests:: fleet_list_ --test-threads=1
cargo test --test guidance --locked --offline
```

The first repaired stable GNU run passes five library cases and seven actual
CLI/read-only-acceptance cases. Controls preserve exact empty and tabular
outputs, configured rather than environment-overridden models, unknown default
capacity, task hints, canonical/subdirectory/linked-worktree scope, no writes,
custom credential redaction and exclusion of notes/key declarations.
Malformed, directory, hardlink, actual sharing-denial and over-limit sources
fail without private content or paths. The exact 65,536-byte file succeeds;
one additional byte fails. Unknown flags and unimplemented fleet mutations
refuse before private settings access. The real CLI runs with an empty `PATH`;
bad tool, endpoint-config and installation selections do not affect listing.

Initial new fixtures incorrectly used single-quoted empty text outside the
catalog's restricted YAML and expected typed error codes in the CLI's
message-only stderr. Those failures are preserved. The fixtures now validate
their source and assert the exact existing stderr contract plus typed codes
through the public API; production CLI error formatting was not changed.
Read-only acceptance runs the actual binary and its unknown-option control.

Terminal controls/Unicode line separators are rejected as invalid single-line
metadata. Internal lone-surrogate text remains lossless; console conversion
fails explicitly instead of silently substituting characters. Guidance checks
also retain the shared Tokio skill's cleanup-quarantine learning and its
existing 40-line limit. No discovery, live model, GitHub, vault access, default
local-delivery replay or full-suite pass is implied.

The pre-review combined affected matrix passed **128 cases per compiler** on stable GNU
Rust 1.99.1 and GNU Rust 1.85.0: 74 library cases, 45 real integrations, six CLI
cases, two guidance cases and one selection control. Both pass format checking
and strict all-target Clippy. Both discover **743 tests**, not 743 executed
passes. Optimized stable passes **65 cases**: 11 status/fleet units, all 45
integrations, six CLI, two guidance and the selection control. The default
local-delivery campaign remains **93/100**, with its seven existing guard
failures and original assertions/deadlines unchanged.

Independent checkpoint review found a Windows environment-case mismatch in
fleet credential redaction. The actual-CLI lowercase-alias control and a typed
ambiguous-alias refusal control both failed before the fix. Listing now reuses
the existing `config::environment_entry` resolver rather than an exact-name
lookup. Both GNU compilers pass **23 targeted post-repair checks**: eight
config/fleet units, all nine fleet integrations and six unchanged CLI cases,
plus strict all-target Clippy and format checks. Both now discover **745**
tests, not a full executed suite. No real credential was used in the controls.

## Native builtin chat controls

Product trace: FEATURE_SPEC 5.2/5.4/5.6/5.8; this verifies a request wrapper and
native planning callers, not complete delivery-seat or outer CLI parity.
The focused selectors are:

```powershell
cargo test --lib llm::builtin_tests --locked --offline
cargo test --lib llm::metadata_drop_tests --locked --offline
cargo test --lib planning:: --locked --offline
```

The tests use the actual builtin wrapper with recorded native `Transport`
implementations and an owned HTTP/SSE loopback server. They verify exact
outbound messages, baseline inclusion, UTF-16/lone-surrogate preservation,
effort/caps/thinking/streaming, selected credentials, declared capability prior
and locked versus actual served labels. They distinguish zero, absent and
partial usage, aggregate overflow, actual HTTP ceilings, unsupported finishes,
one retry per request and three per chat, docs continuation and reasoning-only
cap growth. Observer failures, hanging observers, deadlines, cancellation and
failed retries cannot manufacture success or a usage total.

Four gated real wrapper requests exercise a shared two-slot FIFO across direct
and convenience APIs. Additional controls cover separate profiles, unprofiled
requests, default capacity, declared increases/decreases, unpolled reservations,
holder-bound double release, queued/just-granted future cancellation or drop,
active-body drop and observer failure immediately after a grant. Tokio's
existing paused clock verifies the immediate/30-second heartbeat cadence and
redacted live depth without wall-clock sleeps or detached helpers.
Invalid retry hints are rejected before queue observers can turn them into
timeout guidance; the existing native retry-command validator is reused.

Two metadata Drop regressions were reproduced before repair: a dropped second
actual send exposed the first length response's `{prompt_tokens: 2,
completion_tokens: 0}` as the aggregate, and cancelling then immediately
dropping a hanging completion observer exposed its newly revoked snapshot.
Readiness comes from the actual native body read or observer entry through
owned channels, not a sleep or proxy. Five additional cases cover those exact
sequences plus plain Drop, first cancellation reasons, virtual deadline Drop
and return, absent/prior response evidence, returned non-cancellation observer
errors, healthy zero/unknown recovery, body/slot cleanup, persistent length
budgets and actual-send ceilings. The prior 358 test cases are unchanged.

Planning tests reproduce the formerly missing baseline on an actual outbound
request, share admission across two task drivers and verify critic length and
observer failures retain distinct measured evidence without repair or planner
identity replacement. The earlier planning, pre-plan lease, context and
execution negative controls remain in the ordinary Cargo gates.

These are Windows GNU native tests and owned-loopback/recorded endpoint
observations. No external model endpoint, Node Roster oracle, live hardware
campaign or production concurrency/throughput claim is involved. One
invocation must supply the same admission registry to all its chats; the
tests do not claim cross-process capacity coordination.

The checked chat checkpoint passes 358 all-target tests on Windows GNU with
stable 1.99.1 and Rust 1.85.0: 297 library, 53 backend, six CLI and two guidance
cases, with zero failures or ignored tests. All-target strict Clippy and format
checks pass on both compilers. The owned Cargo fixtures use each matching
installed toolchain; Node consumer fixtures use 20.19.5. Six optimized CLI tests
and standalone optimized empty-PATH help, exact JSON and invalid-option
controls pass. The earlier 324 cases retain their assertions; only 17 required
admission-handle initializer properties were added across four existing test
files, with property-only byte preservation checked.

The metadata Drop follow-up passes 363 cases on both compilers: 302 library,
53 backend, six CLI and two guidance cases. Five new tests were added; all
earlier 358 test sources remain byte-identical. Formatting, all-target strict
Clippy, matching-toolchain Cargo fixtures and optimized/standalone empty-PATH
CLI controls pass. The first MSRV full run retained one unchanged Docker
protocol deadline failure (52/53 backend); the exact isolated rerun passed in
14.66 seconds and the subsequent complete 363-case MSRV rerun passed without
changing any backend limit, assertion or test-thread setting.

## Native inner seats

Product trace: FEATURE_SPEC 5.4/5.5, with 5.1/5.2/5.6/5.8 and sections 3/7/8.
The focused selector is:

```powershell
cargo test --lib seats::tests --locked --offline
```

The component adds **46 native Rust cases** alongside the original 363 cases;
old expectations and dependency pins are not replaced by scripted expectations.
The new tests drive the actual `seats::Driver`, builtin chat, strict decoder,
owned filesystem/tools and default finalizer rather than a policy-only stub.
Owned Node 20 and matching-toolchain dependency-free Cargo consumers exercise
real read/edit/write/delete, scoped/full checks, named counterfactual red/green,
separate author self-review and exclusive RESULT, followed by fresh read-only
review requests, strict pass/fail and owned REVIEW.

An actual loopback HTTP/SSE server drives both seats, including a reviewer
search request, without external endpoints or function tools in review.
Recorded native transports exercise malformed calls, normalized reused IDs,
private/protected denial, turn/context/repair/repeat/no-op handoffs, missing
prerequisites, source-grounded Design, checklist closure invalidation,
README Status, content refusal/correction, committed human hook correction,
qualified outside exceptions and retained failed-full exits after a real
scoped pass. Review controls cover invalid/duplicate/missing check maps,
previous findings, changed-file defects, weaker planner checks, invented Ask/
source grounds, write requests, read-round exhaustion and late human evidence.
Additional controls reject invalid actual coder/auditor/reviewer identities
without substituting a declared model, retain actual zero usage on failure,
and cover an Ask-required absent file omitted from planned scope and an unmet
check hidden behind the word `pass`.

Actual body-read/child-readiness barriers cover cancellation followed by
immediate future Drop, owned body/admission release, request timeout,
settled observer failure and measured model/zero/unknown usage. Pinned live
Windows parent/worker process handles test a dropped actual `run_test` future:
owned processes stop but the writer remains quarantined for explicit recovery.
This is not a claim that dropping a future proves releasable ownership or
Unix containment.

Native context refresh and required-recording failures exercise narrow
identity-checked artifact rotation while preserving a real human REVIEW.
An obvious non-credential sentinel is supplied through a real secret environment
context; the actual write is refused and one model correction succeeds.
The real machine-history/curated-memory APIs verify known zero usage, no
source/sentinel persistence and matching-seat context loading. Promotion is an
explicit host choice, not automatic transcript ingestion.

The new loopback test initially exceeded its 120-second whole-flow fixture
guard under the parallel 42-case load; its unchanged isolated rerun passed in
23.10 seconds. That outer workflow hang guard is now 600 seconds, separately
from the real request timeout/ownership controls. No existing backend timer,
assertion or test-runner thread count was changed. This is not a transport
latency or throughput claim.

These are inner-seat/library checks. They do not test a working delivery CLI,
outer rerouting/review repair, GitHub publication, a live model or Docker
daemon, hardware work, automatic journal recovery, strict Unix execution or
inline-Rust counterfactual extraction.

### Checked inner-seat matrix

Windows GNU, stable **1.99.1** and **1.85.0**, both exit zero:

```powershell
cargo +stable fmt --all -- --check
cargo +stable clippy --all-targets --locked --offline -- -D warnings
cargo +stable test --all-targets --locked --offline -- --quiet
cargo +1.85.0 fmt --all -- --check
cargo +1.85.0 clippy --all-targets --locked --offline -- -D warnings
cargo +1.85.0 test --all-targets --locked --offline -- --quiet
cargo +stable test --release --test cli --locked --offline -- --quiet
```

Each complete debug run passes **409 tests**: 348 library, 53 backend,
six CLI and two guidance, with no failures or ignored tests. The release
selector passes six cases. Standalone optimized help, exact argument JSON
(`["two words","last"]`) and unknown-option refusal also pass with empty
`PATH`. `ROSTER_TEST_CARGO_TOOLCHAIN` matches each outer compiler (`stable`
or `1.85.0`); `ROSTER_TEST_NODE` selects Node 20.19.5. Normal system
`TEMP`/`TMP` are unchanged. No external endpoint, daemon, image acquisition,
GitHub operation or hardware campaign was run.

Focused development first exposed duration-bearing TAP fingerprints that
failed to recognize repeated failures and canonical Windows Cargo-image
preflight handling. Those were repaired without weakening source/identity
checks. An in-repository notebook write also invalidated protected audit
evidence; the seat API now uses the existing repository-bound machine store
outside the worktree, with only explicit compact-summary promotion.
Additional source review tightened actual RESULT-write identity and retained
failed handoff scopes/observations and invalid-response model/usage.
No original 363-case assertion, backend deadline, dependency pin or runner
thread setting was relaxed.

### Focused inner-seat review regressions

Trace: FEATURE_SPEC 5.4/5.5 and 5.6/5.8, subject to sections 3/7/8.
The `df4311a9` findings were reproduced by native execution **before**
production repair:

```powershell
cargo +stable test --lib seats::tests::focused_review::review_repro --locked --offline -- --quiet
cargo +stable test --lib --locked --offline -- seats::tests::focused_review::review_repro_tool_role seats::tests::focused_review::review_repro_legacy_function_call --quiet
```

The first run exited 101 with **0/3 passing**: the exact
`Use web_search to verify the prerequisite, but do not edit unrelated files.`
Ask wrote its document and finalized Pass; a prewarmed real Cargo/libtest
consumer ran eight unchanged failing checks and reached TurnBudget rather than
repeated-failure stop; and a user-role author response finalized Pass after
real product mutation and matching tests. The second run exited 101 with
**0/2 passing**, independently demonstrating clean JSON in a tool role and
alongside a legacy `function_call` also yielded Pass.

After the first root repairs, the focused selector passed **13 native cases**:

```powershell
cargo +stable test --lib seats::tests::focused_review --locked --offline -- --quiet
```

Its table and actual-seat controls cover unrelated negation, negative/mixed
search/fetch clauses, Markdown/documentation/literal mentions and disabled
internet configuration without adding an internet grant. Owned dependency-free
Rust consumers produce real Cargo/libtest timing variation, actual repeated
failures, fixed single-file repair bounds and meaningful assertion progress
through extensions to real green. Literal assertion durations and changed
failure IDs/counts/profiles must not collapse into the same fingerprint.

Direct shared author acceptance and the configured coder both reject invalid
roles and function/tool envelopes. Model, known zero/actual output usage and the
first authority error are retained. Attaching such a report to a default
Attempt cannot make it pass. Normal assistant clean reports, explicit genuine
non-cancellation content-parse/transport advisories, advisory-adapter refusal of
authority errors, cancellation and the earlier source/critic/Drop controls keep
their distinct contracts.

All prior **409 assertions** remain. One existing positive initializer in
`rust/execution/composed_tests.rs` only adds `role: "assistant"`; removing that
one property reproduces its original SHA-256:
`3e0089702716d96b16abddc062be66f2a1f0d4905259bc50280471bcadee94b6`.
No assertion, dependency pin, transport/builtin metadata-Drop source, other
protected assertion, workflow, policy or vendor source was changed. The
existing seat test module only registers the additional focused module.

The first writer-run repaired checkpoint passed **422/422** on both Windows GNU
stable **1.99.1** and **1.85.0**: 361 library, 53 backend, six CLI and two
guidance cases, with no failed or ignored tests. Both compilers pass the
format and strict all-target Clippy commands in the checked matrix above.
The matching-toolchain owned Cargo fixtures pass on each compiler, Node
consumer fixtures use 20.19.5, release CLI six pass and standalone optimized
help, exact JSON and invalid-option controls pass with empty `PATH`.

The first coupled-control pass also exposed the generated Markdown Task-title
form of documentation mentions; that invocation-local selector case is now
covered alongside the original Ask. No production timeout, repair cap,
dependency pin, original assertion or test-runner thread setting was relaxed.
Outer delivery CLI/orchestration and the previously documented platform/live
proof gaps remain incomplete. These writer-run results preceded the independent
follow-up below; they do not claim full-rewrite completion.

### Coupled web-coordination and Cargo-identity follow-up

The independent 422-case campaign **failed with exit 101**: 360 library
cases passed and one failed; the other targets were not reached. The actual
Cargo repro reached `RepeatedFailure`, but observed **four failures rather
than the required three**. Its request-count assertion was not reached.
That campaign is not a passing result or an established transient. Its raw
Cargo outputs were not recorded, so the later diagnostics do not reconstruct
its exact inputs.

The remaining web case was reproduced before its repair:
`Document the result, but first use web_search; do not edit other files.`
The actual configured native seat wrote its guide and returned Pass instead
of stopping Unavailable before any model call or product write. The generated
`# Task:` form carries the same regression. The selector now recognizes an
independently coordinated invocation with bounded sequencing/adverb modifiers,
rather than requiring the prefix to end literally with `and`, `but` or
`then`. Documentation's inherited `how to` complement, copied examples and
tool-local negation remain nonexecuting. No internet capability was added.

For Cargo, bounded diagnostics record the actual retained output, full
normalized-identity hash, normalization completeness and successive
differences. An unchanged isolated repro observed three equal identities;
that did not clear the independent failure. An additional owned native
fixture then invalidated only its external Cargo fingerprint cache after
prewarming. It reproduced **four observations, A/B/B/B**, with the same
`fixed_native_failure` assertion (`left: 1`, `right: 2`). The complete output
differences were Cargo `Checking`/`Compiling` activity and already-normalized
elapsed records, not product or assertion progress.

After filtering build activity, the concurrent focused campaign still
**failed: 15/18 passed**. The original repro observed **A/B/B/C/B** and the
controlled rebuild repro **A/B/A/C/A**, each five failures. Complete normalized
differences identified changing multiplicity/placement of
`Blocking waiting for file lock on package cache`; the fixed-budget control
also failed. All ten recorded outputs and normalized identities were complete;
removing just those exact wait-status records yielded one identity per fixture.
These observations explain concrete false-progress mechanisms, not an assumed
timing flake.

Failure identity is now channel-aware. Only for an actual native result with
a complete failed libtest summary, it excludes exact Cargo build-activity
records and known package/build/artifact lock-wait status records from
**stderr**. It retains the raw observations and actual exit. Assertion stdout,
failed IDs/counts/profiles, warnings, real lock/compiler errors and incomplete
or non-native failures remain identity inputs. Controls distinguish identical
status-looking assertion text from Cargo status. Elapsed normalization,
repair/repeated-failure limits, request counts, concurrency and deadlines are
unchanged.

The repaired focused selector passes **18/18 native cases**, including both
real Cargo repros with exactly **three failures and four model requests**.
The prior 422-case assertions remain; five new cases cover the exact native
web failure, coordination controls, a real cache rebuild and channel-aware
identity controls. This focused result precedes the full-campaign history
below. Outer CLI/orchestration and the earlier platform/live proof gaps remain
incomplete.

The first follow-up stable full matrix passed all **427 cases**. The first
follow-up MSRV full matrix exited **101**, despite all **366 library** and
**six CLI** cases passing: the unchanged backend target passed **52/53** and
failed
`docker_owned_deadline_overflow_and_cancellation_cleanup_do_not_accept_late_success`.
It returned `ROSTER_PROCESS_CLEANUP` with an unverified owned-container cleanup
and a retained `Docker verification deadline expired` cause; guidance was not
reached. The fixture's failure output does not identify which mode/command
expired, and its ordinary Drop removes fixture state. No deadline,
concurrency, protected backend source or assertion changed. This failure is
retained rather than classified as an established transient.

The unchanged isolated selector passed **1/1 in 21.97 seconds**:

```powershell
cargo +1.85.0 test --test execution_backends docker_owned_deadline_overflow_and_cancellation_cleanup_do_not_accept_late_success --locked --offline -- --exact --nocapture
```

That result does not establish the initial failed phase or cause. One unchanged
coherent full MSRV recheck followed it, with bounded read-only observations of
owned project-local fixture phases and no daemon or product/test changes.

The final coherent follow-up checkpoint passes **427/427 native all-target
cases on each compiler**, stable **1.99.1** and Rust **1.85.0** on Windows GNU:
**366 library, 53 backend, six CLI and two guidance**. Both pass format and
strict all-target Clippy with the exact matrix commands above and matching
owned Cargo fixture compilers. No test is failed or ignored in those complete
runs. Optimized CLI **6/6** and standalone optimized empty-PATH help, exact
`["two words","last"]` output and invalid-option refusal also pass. The
earlier MSRV cleanup failure remains historical evidence with its initial
cause unestablished, not a defect declared repaired by this component.

No earlier assertion, repair limit, production/test timeout, runner setting,
dependency pin, protected backend/transport/builtin/metadata-Drop source,
policy, workflow or vendor pin changed in this follow-up. The only
previously approved positive-helper change remains the additive assistant-role
property with byte-for-byte removal proof above. Writing stops at this native
component checkpoint for independent review; it does not complete the outer
delivery CLI, platform proof or full rewrite.

## Native invocation, start and local delivery

Product trace: FEATURE_SPEC 5.4/5.5/5.7/5.8, 5.1/5.3/5.6 and sections 3/7/8.
Focused commands:

```powershell
cargo test --test local_delivery --test cli --locked --offline
cargo clippy --all-targets --locked --offline -- -D warnings
```

`tests/local_delivery.rs` and its owned support modules exercise the real
binary and library, not flag-only handlers. Private fixtures under the project
contain their own configuration, Git objects and consumer sources. Child
environments are explicit/minimal; tests do not read real operator settings
or keys. Node code is only an independent consumer fixture, never copied Node
Roster production. Cargo consumers are dependency-free, locked/offline and
use an owned target outside their worktree with the matching installed compiler.

Configured positive CLI paths run an owned loopback HTTP/SSE service through
actual planning/critique, read/edit/matching checks, RESULT and fresh read-only
REVIEW. Both Node and Cargo consumer paths execute real checks and named
counterfactuals. A private named critic has its own selected environment key/
profile; the next coder request must return to the primary route. Native
scripted transport controls independently verify actual served identities,
measured zero usage, no reviewer function tools and source-free session logs.

Controls cover validated defaults, target/installation/canonical-linked
precedence, invalid/unreadable/hard-linked settings, blank/malformed overrides,
private config/fleet replacement, no cwd/environment mutation and secret/route
redaction. The binary tests check exact exclusive/idempotent init output,
human policy/workflow preservation, offline doctor, invalid/conflicting/
unavailable flags before effects and truthful existing-TASK/Ask stubs.

Real local bare repositories exercise approved local fetch, trunk/current/
complete-ref resolution, offline freshness, ambiguity/stale refs, ahead/behind
drift, malformed/colliding inventory, dirty human source and unrelated human
worktrees. Local submodule fixtures replace declared URLs with an unreachable
non-service label; only explicit approved local HEADs may materialize.
No test updates the real origin or fetches a submodule URL.

Actual root-lease/boundary transfer checks identical held lock bytes and
concurrent writer refusal before/after handoff. Model-body readiness barriers
exercise awaited cancellation and immediate future Drop, retained external
ownership and released shared admission. A real owned pre-plan Node child/
Windows process handle proves termination after a dropped hook while its writer
remains quarantined. A controlled CLI kill at actual model-body readiness leaves
partial context and a lock; only OS-backed proof of that process's death permits
reclaim, and foreign artifacts remain untouched. This is one controlled crash
position, not exhaustive hard-kill/recovery UI coverage.
Additional controls retain uncertainty after dropping a pending native metadata
future in the caller-owned state (not a physical process-readiness proof), and
watch the actual completion log to refuse an awaited source-mutation window
after the final private review gate. Dependency checks occur before that gate,
not after reporting completion.

The actual binary tests its whole-task deadline and exit 124. Ctrl+C is wired
through Tokio and awaits the same cleanup path; noninteractive Windows tests do
not claim physical console-event injection or Unix signal/containment proof.
Invalid/failed reviews, unavailable engines, dirty source, foreign artifacts,
retired Node Roster baselines and implicit PATH authority cannot return verified
delivery. Existing failed-full/private-receipt controls are retained unchanged.

Focused development retained these concrete failures and fixes:

- A wrapper's leading Git `-c` arguments broke `check-ignore` literal-pathspec
  handling; direct ignore preflight now uses the existing dedicated path.
- Owned Windows sockets inherited nonblocking mode in the test server; accepted
  sockets are made blocking before bounded HTTP reads.
- Actual debug `roster` code delivery overflowed the Windows main stack.
  A broad pass exposed remaining inner coder/reviewer frames after outer
  boxing. Heap-pinned invocation and inner-seat boundaries fixed it without a
  new runtime, larger global stack or weaker checks. The new whole-workflow
  model-readiness guard is 600 seconds, independently of unchanged production
  deadlines, physical readiness assertions and earlier test timeouts.
- The new hook fixture initially exceeded the existing 60000 ms manifest limit;
  it now uses that real limit. Review-invalid controls supply the existing one
  correction response, not a network failure from an exhausted fake server.
- Stub checks pin the existing exact `AI-Run: not emitted for a deterministic
  stub.` bytes, rather than confuse that truthful label with emitted provenance.
- The named-critic fixture initially supplied a field outside the existing fleet
  schema; the fixture now uses the unchanged validator. A separate sentinel
  control caught that generic evidence redaction does not apply configured route
  labels: CLI diagnostics now also use the native configured-string redactor.
  Windows case-insensitive selected environment names and malformed Unicode have
  explicit controls, without mutating the process environment.
- Source review found a dependency recheck after the terminal private gate and
  bootstrap Git uncertainty scoped inside the dropped future. They now remain
  ordered before the last gate and retained in external invocation ownership,
  with dedicated native cancellation/completion controls. Origin fetch refmaps
  are fixed to origin tracking refs, not a configured human-branch destination.

The only earlier CLI expectation changed is its intentional help-surface
sentence; the existing GitHub `run --issue` unsupported control still fails
before effects. No older acceptance assertion, Cargo/Node coverage rule,
timeout, model/usage check, dependency version or receipt seal is relaxed.

These controls do not implement or accept GitHub issue/publication, real vault
storage, outer review repair/routing/rescope/waves, automatic learning/history/
cleanup, strict Unix execution, inline Rust counterfactual extraction, live
external models/GitHub/Docker/hardware, cross-platform release or the frozen
reliability campaign.

### Verified local-delivery matrix

The initial checkpoint's frozen native source passed on **Windows GNU**,
before the independent-review follow-up below:

| Compiler / profile | Command gate | Actual result |
| --- | --- | --- |
| stable Rust 1.99.1 / debug | format, strict locked/offline all-target Clippy, full all-target tests | **463 passed**, zero failed/ignored |
| Rust 1.85.0 / debug | format, strict locked/offline all-target Clippy, full all-target tests | **463 passed**, zero failed/ignored |
| stable Rust 1.99.1 / release | CLI plus complete local-delivery integration targets | **42 passed**, zero failed/ignored |

Each full debug matrix is **366 library + 53 backend + 36 local-delivery +
6 CLI + 2 guidance** tests. The earlier 427 cases remain; new local coverage
adds 36. Release coverage is six existing CLI cases plus all 36 local cases,
not a claimed full optimized all-target matrix.

```powershell
cargo +stable fmt --all -- --check
cargo +stable clippy --all-targets --locked --offline -- -D warnings
cargo +stable test --all-targets --locked --offline -- --quiet
cargo +1.85.0 fmt --all -- --check
cargo +1.85.0 clippy --all-targets --locked --offline -- -D warnings
cargo +1.85.0 test --all-targets --locked --offline -- --quiet
cargo +stable test --release --test cli --test local_delivery --locked --offline -- --quiet
```

`ROSTER_TEST_CARGO_TOOLCHAIN` matched the outer compiler (`stable` or `1.85.0`);
Node consumer fixtures selected installed **20.19.5**. Normal `TEMP`/`TMP`
were unchanged. The standalone optimized binary also passed help and exact
`["two words","last"]` argument splitting with empty `PATH`; the integration
targets additionally exercise offline init and existing-TASK stub with explicit
images and no PATH discovery.

The session aggregate native-source fingerprint before/after the serial gates
was `A328E4396576AAFAD8FA588241E1473B2EAF3D3C7A193E12D32C2514813CA898`.
It includes checkout paths and is a local frozen-input check, **not** a portable
release checksum or signed acceptance. No gate ran concurrently with another
final compiler gate. Earlier diagnostic failures above are not counted as passes.
Policy, workflows, the vendor tree/gitlink and Cargo dependency pins were not
changed; only the already-locked Tokio signal feature was enabled.

## Original reviewer 49440ec5 regression campaign

The parent independently finished the prior stable format/strict all-target
Clippy/**463 native tests** with exit zero and quiesced before this repair
handoff. Four source-confirmed findings were then given native failing controls
**before** behavioral repair. The first run was **five cases: four failed,
one passed**:

| Original finding | Actual pre-fix observation |
| --- | --- |
| New-tree writer released on uncertain submodule cleanup | Real approved Git clone **and** checkout completed behind an owned native readiness process. Dropping a second actually running owned process made the shared Git state uncertain. Continuing the phase removed `run.lock`, while only `prepare.lock` remained. |
| Nonheuristic selected credential leaked | An ordinary non-credential-shaped value selected through `CRITIC_AUTH` appeared in actual generated ASSIGNMENT.md, TASK.md and CONTEXT.md. |
| Keyless critic rejected | A real configured Ask CLI with a valid fleet profile lacking `api_key_env` failed before requests with “Selected critic profile needs an API key environment name”. |
| Normalized authority leaked | A real owned loopback HTTP body timeout produced `ROSTER_LLM_TIMEOUT`; the public invocation diagnostic still contained its configured host:port. |

The pre-existing-style actual callback control already redacted its selected
value at the low-level client. That pass was retained, not presented as proof
that assignment/artifact and non-transport error paths were protected.

Eleven additional native cases now cover the repairs. The phase fixture is a
dependency-free Rust 2024/minimum-1.85 program built and strictly linted with the
matching installed Cargo, locked/offline. It delegates the absolute installed
Git image for actual repository/clone/checkout behavior, then exposes explicit
process and descendant readiness barriers. Its control code is not passing-test
authority or a simulated cleanup receipt. Success/known exit failure/awaited
cancellation/immediate Drop are checked at **both** clone and checkout, including
the caller-owned real lease, runtime `run.lock` and second-writer denial.
Signalled process handles never clear a quarantined lease.

Credential controls test the actual builtin profile-to-transport path with
owned HTTP authorization observations: keyless optional, inherited configured
name, custom `CRITIC_AUTH`, missing required key, blank and invalid values.
Unselected invalid environment entries must remain unread. The ordinary
selected values also exercise native TASK/RECIPE/ESTIMATE/PLAN writer checks,
real filesystem error diagnostics, assignment refusal and no log/error leak.

Owned HTTP JSON/SSE endpoints exercise real timeout and stall timers. Native
controlled-body and callback failures cover primary/effective critic hosts,
domain case, nondefault and explicit/default ports and IPv6 normalization,
without resolving or contacting those non-loopback metadata labels. The CLI
timeout control checks stderr and the persisted unverified RESULT as well as
known cleanup. Actual served model and measured zero usage remain assertions.
Original error names/codes, including an absent code, must remain unchanged.

Two development controls initially used the raw OpenAI client instead of the
existing builtin `api_key_env` adaptation, and caught an invented fallback code
on an otherwise uncoded callback error. They now use the real builtin path and
preserve absent codes. A combined local run exposed only the new native phase
workflow's 120-second readiness guard under startup load; that outer fixture
guard is now 600 seconds. Production Git/hook/request deadlines, the actual
readiness/handle assertions and all prior 463 test assertions are unchanged.

The first repaired MSRV full run retained a separate unchanged legacy-control
failure: `execution::owned::tests::owned_job_must_wait_for_descendants_after_root_exit`
had **365 library passes and one failure**, parsing empty captured stdout for the
descendant PID after its existing two-second timeout. The exact unchanged case
then passed in **2.18 seconds**. Its source, assertion, production timeout and
runner thread count were not edited. That failed campaign is retained
separately from the subsequent coherent full MSRV campaign, not called green
or silently replaced by the isolated pass.

Only required ownership initializers changed in earlier local tests:
`start::prepare_local` receives an external `Preparation`, and callers close
that holder rather than a returned `Prepared.lease`. This does not replace a
private receipt with JSON, infer death from age, or grant new permissions.
Source, pins, policy/workflow/vendor boundaries and removed-Node restrictions
remain binding. The repaired source is handed back for **the original
reviewer's independent confirmation**, not outer repair/routing/GitHub scope.

### Verified repaired matrix

After the four original findings were reproduced and repaired, the frozen source
passed these **serial Windows GNU** gates:

| Compiler / profile | Result |
| --- | --- |
| stable Rust **1.99.1** / debug | format, strict locked/offline all-target Clippy, **474 native tests passed**, zero failed/ignored |
| Rust **1.85.0** / debug, coherent full repeat | format, strict locked/offline all-target Clippy, **474 native tests passed**, zero failed/ignored |
| stable Rust **1.99.1** / release | **53 CLI/local tests passed**, zero failed/ignored |

Each complete debug run is **366 library + 53 backend + 47 local-delivery +
6 CLI + 2 guidance** cases. This preserves all prior 463 and adds 11 review cases.
The optimized run includes the original 42 CLI/local cases and all 11 additions;
it is not a claimed full optimized backend/library matrix.

```powershell
cargo +stable fmt --all -- --check
cargo +stable clippy --all-targets --locked --offline -- -D warnings
cargo +stable test --all-targets --locked --offline -- --quiet
cargo +1.85.0 fmt --all -- --check
cargo +1.85.0 clippy --all-targets --locked --offline -- -D warnings
cargo +1.85.0 test --all-targets --locked --offline -- --quiet
cargo +stable test --release --test cli --test local_delivery --locked --offline -- --quiet
```

Consumer fixtures used the matching installed `stable`/`1.85.0` Cargo toolchain
and Node **20.19.5**, with normal TEMP/TMP. Native Git phase fixtures were
dependency-free, formatted, strictly linted and built locked/offline. Standalone
optimized help and exact `["two words","last"]` argument splitting additionally
passed with empty PATH. Git/consumer operations still require explicit images.

The before/after session native-source fingerprint was
`CB6519280ED3BD2986FB722E9C7E409CAE81C8D784911C19634DDAE7EF4D0FA2`.
It is the same checkout-path-dependent frozen-input check described above, not
a portable release checksum. Source was not edited and no competing compiler
gate ran during these final campaigns. The first MSRV failure, its unchanged
isolated rerun and the final coherent repeat remain distinct evidence.

Policy, workflows, contracts vendor sources/gitlink and Cargo manifest/lock pins
were unchanged by these repairs. There were no new dependencies, Node Roster/
oracle/frozen checkout, application comments/unsafe/FFI, publication or external
model/GitHub/Docker/hardware calls. Windows ACL, strict Unix/MSVC behavior,
physical Ctrl+C injection, outer orchestration and human acceptance remain
outside this proof. Hand back for original reviewer **49440ec5**, not a new
feature scope or an invented review approval.

## Non-HTTP observer persistence regression

Original reviewer **49440ec5** confirmed the lease, all-selected-credential
and effective-critic fixes after the parent independently passed the coherent
**474-case stable matrix**. Its remaining source finding was a non-HTTP error
boundary: completion callbacks were protected, but the first `seat-tool`
observer and `seat-stopped` notification could return raw selected metadata
before terminal artifact construction.

The actual native delivery pre-fix run failed **both new cases, zero passed**:

- A valid response-backed completion executed an owned `read_file`, then the
  first real seat-tool callback failed. The persisted RESULT “First failure”
  exposed configured diagnostic metadata.
- A genuine one-turn/no-progress failure was retained, then the coder's real
  stop notification failed with selected primary/critic diagnostic metadata.
  The returned terminal notification annotation exposed that metadata.

Those tests precede the behavioral fix. The shared seat-driver error sanitizer
now runs before propagation from both methods, including timeout, authority
and cancellation branches. The first tool failure retains its original
name/code and nested uncoded cause; a subsequent stop failure cannot replace
the original turn-budget error. The same source boundary serves both reviewer
notification call sites; no separate CLI render rule is the protection.

Six added current-thread native delivery cases use real local owned Git
worktrees and the existing scripted native transport. Configured primary and
critic HTTPS authorities, mixed-case host:port, full URLs, diagnostic model
labels and ordinary values selected by arbitrary `FIRST_AUTH`/`CRITIC_AUTH`
names are exercised without external requests. Coder measured model identity
deliberately equals a configured diagnostic model label: diagnostics must hide
it while actual AI-Run metadata must retain it. Fresh reviewer identity and
measured zero usage remain independently asserted, including invalid strict
review responses followed by failed stop notification. A healthy real
implementation/review still passes; model advice and an accepted notification
do not convert missing implementation into a pass. A real pending seat-tool
callback competes with cancellation and a ready successful return: the first
17-ms timeout reason wins, the callback future drops, no late success is
accepted and the owned writer closes.

The initial post-fix targeted run had **five passes and one new-control
expectation failure**: that new cancellation test expected a timed-out header,
whereas the existing terminal API renders cancelled/unverified with retained
`ROSTER_TASK_TIMEOUT` and its original budget. Only this new expectation was
corrected; the exact native case then passed. No production cancellation
contract, prior assertion, timeout or runner thread count changed.

### Verified observer-boundary matrix

The same executable/test inputs pass these serial Windows GNU campaigns,
using the locked/offline all-target commands above and matching installed
consumer toolchains:

| Compiler / profile | Result |
| --- | --- |
| stable Rust **1.99.1** / debug | format, strict all-target Clippy, **480 native tests passed**, zero failed/ignored |
| Rust **1.85.0** / debug | format, strict all-target Clippy, **480 native tests passed**, zero failed/ignored |
| stable Rust **1.99.1** / release | **59 CLI/local tests passed**, zero failed/ignored; empty-PATH standalone smoke passed |

Each full debug run is **366 library + 53 backend + 53 local-delivery +
6 CLI + 2 guidance** cases: all prior 474 plus six new observer controls.
Neither current full campaign needed a retry. Earlier pre-fix, new-control
expectation and legacy MSRV failures remain distinct history, not erased by
these later passes. Release is **6 CLI + 53 local** cases, preserving the
original 53 and adding six; it is not a full optimized library/backend matrix.
Optimized standalone help and exact `["two words","last"]` argument splitting
also pass with empty PATH. Git/consumer operations still need explicit images.

The session before/after native/Cargo/test-input fingerprint is
`56CD952AE4AA1BE36172F376E498942219675804528E07EE3828343D3923CB8F`.
It is a frozen-input check, not a binary/release checksum. No executable/test
source changed and no competing build ran between or during the campaigns.
The native delivery failure evidence is retained in
`.native-review49440-events-prefail.log`, the initial targeted outcome in
`.native-review49440-events-targeted.log`, and the coherent compiler results in
`.native-review49440-events-stable-final.log` /
`.native-review49440-events-msrv-final.log`. Optimized cases and empty-PATH
results are retained in `.native-review49440-events-release-final.log`.

This follow-up changes only the shared seat error boundary, added native
controls and related RUST/TESTING documentation. It does not change config/
start ownership, private seals, Cargo dependencies, policy/workflows/vendor,
permissions or removed-Node restrictions. Strict Unix/MSVC/Windows ACL proof,
physical Ctrl+C injection, outer orchestration and human acceptance remain
outside these Windows GNU gates. Original reviewer **49440ec5** must confirm
the repair before a later scope.

## Independent Docker cleanup failure: retained diagnostic checkpoint

After the observer-boundary producer's two 480-case passes, the parent's
independent stable campaign exited **101** in the unchanged
`docker_owned_deadline_overflow_and_cancellation_cleanup_do_not_accept_late_success`.
The parent reported **366 library + six CLI passed**, then **52 backend passed
and one failed**; all 53 local-delivery and two guidance cases were not reached.
`OwnedProcessCleanupError` refused writer release, retained the container
snapshot, and included a verification-deadline cause. The failure's mode,
create/CID/readiness state and cleanup subphase are unknown. The supplied
container identity was `roster-verify-138144-bc77138291d3c9ec`. No matching
owned fixture directory remained when narrowly inspected; no foreign object
or original parent evidence was deleted during diagnosis.

Focused native diagnosis did not reproduce that failure:

- Exact original stable target, before instrumentation: **one passed**, 16.80 s.
- Instrumented Docker-only selector: **seven passed**, 94.34 s.
- Instrumented backend-only campaign, unchanged parallelism: **53 passed**,
  971.84 s.
- Final exact target with bounded ID/state diagnostics: stable **one passed**,
  22.69 s; Rust 1.85 with matching installed fixture compiler **one passed**,
  19.91 s. Formatting and strict all-target Clippy pass on both compilers.

These later passes establish neither the original cause nor its resolution.
In the backend campaign's timeout observation, the fixture logged image
inspection, creation/CID publication and inspection, followed by full-ID
removal/confirmed absence, without a logged `start` or ready marker. The
original pass assertions held, but this does not prove the unlogged start
process was never submitted or classify the parent's failed run.

`tests/support/docker_diagnostics.rs` adds no gates or grants. It records
bounded protocol phase timestamps/PIDs, full-ID selectors, CID/intent state,
writer-marker presence and typed error classes/digests; it does not log raw
error bodies, arguments, credentials or endpoint metadata. On returned
failure it retains the exact owned fixture root and its evidence. Event/call
capture has explicit bounds/truncation flags. All existing timeout, overflow,
cancellation, object-count, owned-removal and close assertions stay intact;
the timeout remains **5000 ms**, cleanup **10 seconds**, and thread counts
unchanged. Production Docker/owned/backend code and dependency pins are
unchanged. No live daemon/container was used.

That handback's missing evidence was a failing invocation's emitted phase/CID/
error record and retained root. The next parent campaign supplied it as
described below. No focused passing retry was presented as a repair. The first
parent failure and earlier unestablished MSRV histories remain failures, not
flakes erased by later passes.

## Native Docker submission proof and coupled CLI validation

The next original default stable all-target run exited **101** after
**366 library** cases (657.36 s), **six CLI** cases (1.00 s), and **52 backend
passes plus one failure** (976.97 s). Its actual failure is
`.docker-480-evidence/132880-timeout-1792275625280678.json`; the original
`.execution-backend-fixture-132880-22` and all 37 sealed evidence files remain
unchanged. The raw parent campaign log remains in the session files. The JSON's
15.4-second host interval includes preparation and is not the five-second
verification clock.

The uniquely sourced remaining-deadline error and retained native state place
the failure before create backend entry: the old `submitted=true` was set
before that check. An expired-controller control built and actually executed
the owned Rust CLI for the erroneous cleanup lookup and failed **zero passes /
one failure** before the fix. Its log is `rootfix-first-prefail.log`; its root
`.docker-native-admission-141952-f2b4298ff271a091` is retained. Non-event or
name absence is not the new no-effect proof.

The private owned spawn gate now marks possible execution immediately before
native spawn. Three added Windows native cases verify:

- Expiry before the gate remains unavailable but requires no daemon cleanup
  and permits safe writer release.
- Validation/cancellation before entry stays not-attempted; an attempted
  missing executable with no PID remains may-have-executed.
- Actual pinned live create processes are cancelled at an atomically
  published native readiness barrier before object creation or after object
  creation but before CID publication. Unknown creation retains ownership;
  actual owned metadata permits full-ID recovery. Foreign object bytes remain.

Schema-one `submission.json` / `native_create_submissions` diagnostics carry
actual native submission state/PID and production-clock offsets. They are
forensics, never parsed into a cleanup grant or a passing test seal. The
harness also retains returned-error and panic roots and copies native
unavailable-report receipts before the unchanged outcome assertions.

The first repaired full stable campaign passed **369 library / six CLI /
53 backend / two guidance**, then **50 of 53 local-delivery** cases passed;
three configured CLI cases stack-overflowed. MSRV full was not started by
that failed command. `rootfix-stable-full-first.log` and the unchanged isolated
failure `rootfix-local-stack-prefail.log` are retained. This tightly coupled
future-frame regression was repaired by returning one common owned future,
not by changing stack/compiler flags, thread counts, timeouts or old tests.
All three actual Node/Cargo/Ask CLI selectors subsequently passed together
(117.19 s). Initial new-control parse/lifetime errors and strict-lint
needless-borrow/lifetime failures remain distinct diagnostic history.

The next compact-source original-load stable run passed **483 cases**:
369 library (672.53 s), six CLI (1.53 s), 53 backend (1035.57 s), two guidance
and 53 local-delivery (702.37 s). Its same-source MSRV run passed 369 library
(744.11 s), six CLI (2.37 s), then 52 backend cases passed and the original
Docker control failed (1261.42 s); remaining targets were not reached.
`rootfix-stable-full-compact.log` and `rootfix-msrv-full-compact.log` are
retained separately.

This time the retained root `.execution-backend-fixture-79040-21` contains
the actual private-native serialized state: `not-attempted`, null PID,
5422.4542 ms before cleanup, 0.3223 ms cleanup, timeout 5000 ms. Only image
inspection was invoked. The original `rm` assertion therefore correctly
rejected a test that had never exercised create/cleanup. This is not the old
uncertain-owner error or another pass erasing it. The failed root and all
files are sealed in `79040-retained-before-fingerprints.json`.

The evidenced fixture readiness precondition now executes exactly one actual
read-only image query against the same pinned Rust CLI before boundary/timed
verification setup. It uses isolated empty Docker configuration/minimal
environment, verifies unchanged executable identity/size/timestamp, actual
matching Linux image output, empty stderr, exactly one image-inspect call and
zero objects. Setup has its own 30-second bound, separate from the **unchanged
5000-ms real verification**. No timed action is removed, no timeout/cleanup
clock is shifted, and no existing assertion is conditionalized or relaxed.
`fixture_setup` records PID/time and the call boundary; it is not a passing
receipt and the production verifier independently repeats image validation.
The native before-spawn regression still independently covers safe
non-submission instead of hiding that production path.

Every existing 480-case assertion and all dependency pins remain.

### Verified native submission-repair matrix

The same frozen native/Cargo/test inputs pass the original default commands,
without selectors, test-thread options, budget overrides or competing
campaigns. Each invocation's external consumer fixtures select the matching
installed compiler; Node consumers use 20.19.5.

| Compiler / profile | Actual command and result |
| --- | --- |
| stable **1.99.1**, debug | `cargo +stable-x86_64-pc-windows-gnu test --all-targets --locked --offline`: **483 passed**, zero failed/ignored |
| Rust **1.85.0**, debug | `cargo +1.85.0-x86_64-pc-windows-gnu test --all-targets --locked --offline`: **483 passed**, zero failed/ignored |
| stable, optimized CLI/local | `cargo +stable-x86_64-pc-windows-gnu test --release --test cli --test local_delivery --locked --offline`: **59 passed**, zero failed/ignored |

Each debug run contains **369 library + 53 backend + 53 local-delivery +
six CLI + two guidance** cases. Stable target elapsed times were 664.45 /
1081.24 / 845.44 / 1.39 / 0.00 seconds; matching MSRV times were 671.07 /
1019.89 / 872.32 / 1.68 / 0.00 seconds. Optimized CLI/local times were
1.01 / 892.14 seconds. Both compilers pass Cargo format and strict all-target
Clippy with `--locked --offline -- -D warnings`; standalone fixture rustfmt
also passes. Optimized empty-PATH help exits zero, argument splitting returns
exactly `["two words","last"]`, and an invalid option exits one.

Final evidence is `rootfix-stable-full-readiness.log`,
`rootfix-msrv-full-readiness.log`, `rootfix-final-lint-release.log` and
`rootfix-release-empty-path.json` under `.docker-480-evidence`. The precise
verified input checkpoint is `rootfix-readiness-source-checkpoint.json`.
Documentation's result matrix was updated afterwards; native sources were
not changed between either full campaign or optimized verification.
`rootfix-readiness-original-test-preservation.json` reverses only the
additional fixture readiness/retention/receipt lines and test-only native
barrier modes to reproduce both original complete source hashes.

Original `132880` and `79040` roots, their JSON/native receipt and all sealed
bytes remain untouched for independent review. Pre-fix, failed full,
new-control compilation/format/lint and narrow regression evidence remains
distinct from the final passing matrix. No unknown cleanup becomes certain,
no absence/name/PID becomes a grant, and no setup probe becomes verification
evidence. Strict Unix/MSVC/ACL proof, real Docker/daemon containment, physical
Ctrl+C injection, outer orchestration, publication and human acceptance remain
outside these Windows GNU fake-daemon/component gates.

## Native file vault and selected local-run credentials

Product trace: FEATURE_SPEC 5.8, 5.2, 5.4 and 5.5, respecting 3/7/8.
The independent legacy cases were read from `HEAD:tests/vault.test.mjs` and
`HEAD:tests/secrets.test.mjs`; no old implementation or Node oracle is retained.
Current native contracts and exact limits are in
[RUST](RUST.md#native-file-vault-and-selected-endpoint-credentials).

Targeted controls:

```powershell
cargo test --lib vault:: --locked --offline
cargo test --lib secrets::tests --locked --offline
cargo test --test local_delivery vault_controls:: --locked --offline
```

[`rust/vault/tests.rs`](../rust/vault/tests.rs) covers absence without
creation; ASCII/name/value/App/PEM policy; exact default/wire/order/hex names;
NIST AES-256-GCM and independently generated native .NET AAD/wire bytes;
fresh nonces/same master key; wrong name/tag/nonce/ciphertext/key; malformed
JSON/Base64/fields; missing, empty, short and long keys without replacement;
all explicit byte/entry/path thresholds; sorted listings; Git/worktree/link
guards; native ownership/ACLs; failed complete replacement; causeless,
secret-free errors and RAII zeroize type contracts.

Real native child processes start from an explicit readiness/release barrier.
Four case-distinct first writers and two concurrent readers must share only
one complete single-link key and authenticate every observed entry, without
partial/multi-link retries. Separate real child crashes retain incomplete
16-byte and complete 32-byte unpublished temporaries, or a complete published
key. Only the last is reused; existing entries without a key forbid regeneration.
Owned children have bounded waits and kill/wait cleanup.

Windows tests audit actual fixtures through an independent fixture-only BCL
observer: current token SID owner, protected inheritance, exactly one Allow
FullControl ACE. Native controls also exercise owned wide and null DACLs,
denied file/directory reads, concurrent hardening/failure/explicit repair,
recreated identities, real junctions and hard links, pinned root/ancestor
rename denial, and a genuine kernel-sharing atomic-replace failure.
This observer is not a production provider; there is no runtime
PowerShell/`icacls` call. Foreign-owner descriptor rejection is separate from
cross-user filesystem proof: this token cannot assign a foreign owner.

[`rust/secrets.rs`](../rust/secrets.rs) preserves own nonempty environment
precedence without touching a provider, unset/empty fallback, distinct missing
values, non-string/empty returned values and fixed causeless resolution
failures. Whitespace is nonempty; the old stricter environment-only delivery
APIs retain their original assertions.

[`tests/support/local_vault.rs`](../tests/support/local_vault.rs) poisons
unselected/forbidden entries to prove no vault enumeration, checks effective
critic inheritance/deduplication, and rejects corruption before CLI effects,
including optional primary/critic auth. A real compiled local CLI completes
planning, separate authenticated critic, editing and independent reviewer
seats from actual file-vault values; no environment key is substituted.
Its recordings exclude both selected sentinels. Native HTTP wrapper checks
keep response-backed primary/critic models, measured nonzero usage and
unknown usage; persisted coder/reviewer AI-Run fields keep reported zero
counts rather than invented values.

Every shared local fixture now receives an owned outside-Git home before the
CLI starts, including old required-key-negative controls. Existing leaf CLI
fixtures invoke no credential-resolving command. Removing only the additive
home/module glue reproduces both original integration-file byte hashes;
all earlier assertions, budgets, flags and execution inputs remain unchanged.
Failed new fixtures are retained before repair, not cleaned or warmed to pass.

### Verified frozen vault matrix

On native Windows x86_64 GNU, the final frozen input set passes:

| Compiler / profile | Command / result |
| --- | --- |
| Stable **1.99.1**, debug | `cargo +stable-x86_64-pc-windows-gnu test --all-targets --locked --offline`: **523 passed**, zero failed/ignored |
| Rust **1.85.0**, debug | `cargo +1.85.0-x86_64-pc-windows-gnu test --all-targets --locked --offline`: **523 passed**, zero failed/ignored |
| Stable **1.99.1**, release | `cargo test --release --test cli --test local_delivery --locked --offline`: **66 passed**, zero failed/ignored |
| Stable **1.99.1**, release | Targeted `--lib vault::` and `--lib secrets::tests`: **30 + 3 passed**, zero failed/ignored |

Each full run has **402 library + 53 backend + 60 local + 6 CLI + 2 guidance**
cases. The original **483** remain; **40** are added: 30 vault, three resolver
and seven local credential controls. Native worker subprocess checks are not
added again to the parent test count. Both compilers pass `cargo fmt --all --
--check` and strict all-target Clippy with `--locked --offline -- -D warnings`.
Each campaign selects the matching installed compiler in owned Cargo fixtures;
Node 20.19.5 is only the approved consumer-fixture interpreter.

The full campaigns run serially at original default load, flags, budgets and
thread settings. An initial complete stable 523-case run also passed before
the final shared JavaScript-whitespace parity and equivalent hex-formatting
refinements; **both final full runs** use the refined frozen source.
The final source/manifest/test hashes have zero drift across the MSRV, stable
and release sequence. Only documentation results are filled in afterwards.
Detailed counts, target durations, log hashes, exact inputs, original-source
projections and all retained failures are in ignored `.vault-evidence`.

An independent parent run also passed the frozen 523-case baseline, formatting
and strict all-target Clippy. Review still found an unresolved temporary-file
identity gap in name-based publication/cleanup; passing the baseline does not
clear that finding or accept the vault component.

The subsequent lazy credential-map validation repair adds three transport
controls for invalid mapping shapes, environment precedence, valid stored
credentials and optional absence:

```powershell
cargo test --lib llm:: --locked --offline
```

All **69 LLM cases** pass on stable and Rust 1.85, including the three added
cases; formatting and strict all-target Clippy pass on both. The original LLM
test source is byte-identical after removing only the added tests and transport
header capture. Before the repair, the correctly configured new controls passed
both positive cases and failed malformed-map rejection. The initial fixture
used `api_key_env` instead of the raw client's `api_key_name`; that setup failure
is retained separately, without changing assertions. No full 526-case campaign
after this repair, completed vault review, full rewrite, release/distribution,
physical containment or harness acceptance is claimed.

### Retained vault diagnostics and platform limits

Ignored `.vault-evidence` records first failures, repaired controls, dependency
review/acquisition and before/after source hashes. The retained sequence is:

1. Private validator export/WinSafe flag-shape compile errors, then a native
   GNU link failure for `ktmw32`. The GNU-only linker script aliases the real
   locked `winapi` archive; no stub, unsafe FFI, global toolchain change or
   production helper was added.
2. Initial filesystem controls failed because opening new objects requested
   unnecessary `WRITE_OWNER`; ownership verification plus `WRITE_DAC` avoids
   taking ownership. All first PID-139184 fixture roots remain.
3. Native rename control disproved metadata-only directory pins, and a
   PowerShell-module autoload dependency broke the first external audit.
   Directory read-data/no-delete pins and direct BCL observation repair these;
   both PID-136508 roots remain.
4. The new authenticated test endpoint inherited a nonblocking accepted
   stream, causing the first CLI request failure. The fixture now explicitly
   makes the accepted stream blocking, with finite I/O bounds.
   `.native-local-143556-623db675c67986fc` and its dedicated home remain.
5. A later CLI completed successfully but the new assertion incorrectly
   expected the planner model in intentionally minimal session persistence.
   The test now checks the actual persisted coder/reviewer AI-Runs and adds
   separate real-response primary/critic measured/unknown checks.
   `.native-local-127768-2164d51b3f0b51c4` and its home remain. No old assertion
   or product persistence contract was changed to accommodate that mistake.
6. The proposed 32,768-unit path bound failed the real Windows path API.
   Native probing proved 32,766 inclusive; the exact boundary controls and
   documentation now use that threshold. The PID-137168 retained root remains.
7. Strict Clippy caught one new Boolean assertion style issue; its equivalent
   `assert!` repair adds no lint suppression or weakened check.
8. Rust 1.85's strict Clippy additionally rejected per-byte `format!`/collect
   in the filename encoder. Direct standard-library formatting into one
   preallocated `String` preserves exact hex bytes without a suppression,
   custom crypto/Base64 routine or new dependency.

An owned foreign-owner capability probe also remains: the unprivileged OS
token denied assigning another SID. Native descriptors exercise rejection,
but cross-user filesystem ownership/impersonation is not claimed as tested.
Unix/macOS source includes handle-bound UID/mode and path-swap controls but
has not been compiled or executed natively here; Windows MSVC, file-symlink
privilege cases, physical power loss and distribution likewise remain separate
gates. Existing Windows state/settings trusted-host gaps are unchanged.
No live model, GitHub, Docker or hardware campaign, commit or publication ran.

## Fixed-profile outer local continuation controls

Trace: FEATURE_SPEC **5.3**, **5.4**, **5.5**, with sections 3/7/8 unchanged.
The pre-work source freeze includes all expected **526** earlier native cases,
including the parent's three ordinary credential-map shape controls. The
previously accepted full freeze was 523; the three later cases had only their
69-case LLM selector verified, not a full 526 campaign.

`tests/support/local_continuations.rs` adds **33** native cases over the real
local coordinator and actual private coder/reviewer APIs, not caller-created
CoderResult/ReviewResult JSON or unit-only retry arithmetic. Owned loopback
HTTP/SSE serves scripted model messages. Actual Git repositories, one new
registered Ask worktree, native OS writer leases, Node/Cargo consumer checks
and the compiled `roster` CLI provide the effects and independent authority.
Every newly owned configured CLI environment has an owned HOME/USERPROFILE
and injected obvious dummy endpoint keys; no real user vault is consulted.

The controls cover:

- Completed semantic fail/repair/pass, two-repair exhaustion, same-unmet and
  proper-subset changed-strategy continuations, strict prior finding coverage,
  and Original Ask audit failure despite green numbered checks.
  A real Unicode finding crossing the bounded feedback edge must keep valid
  context while the fresh reviewer still requires every original finding.
- Incomplete/malformed/error reviews, genuine arbitrary response failures,
  explicit skip-review, offline stub and existing planning-only behavior.
  Repeated secret-content denial and a failed stopped-seat notification must
  not reset correction budgets or become perspective authority; the latter's
  actual exception is surfaced while the original seat failure stays recorded.
- Actual scope 3 → 6 → 12, 12 → 16, initial 16 and strict zero; exact recorded
  requests, cap/attempt exhaustion as planning escalation, protected/private/
  workflow/vendor refusal, retained edits and review/context/summary events.
- Actual turn exhaustion with only two fixed-profile perspectives, success
  from correct earlier writes without rewriting them, shared admission queued
  in a fresh context, and actual second RootLease refusal throughout the loop.
- Own source regression producing a real failed full suite, a scope handoff,
  scoped green, and both further perspectives. All original full-check counts
  and the three-failure repeated-failure stop remain required; every archived
  and terminal result must retain the actual negative full obligation.
- Exact TASK/RECIPE/ESTIMATE and writer identities, actual artifact archives,
  fresh checklists with revoked positive evidence/reopened unmet IDs, measured
  zero versus unknown usage, and retained author/audit response metadata.
- Caller cancellation and the actual invocation Tokio deadline at an observed
  transition (controlled clock, not a wall-time performance claim); source,
  config, estimate, foreign context and arbitrary observer changes. A real
  valid Git branch change during an HTTP response must revoke pending writes
  before effects and cannot become a perspective reason.
- Invocation-wide actual-send limits across new contexts and review repairs,
  terminal feedback preservation when no request remains, and real configured
  Ask and Cargo CLI repairs with fresh final private reviewed authority.

Two existing bounded negative fixtures explicitly declare their original
request horizons: five/six responses for the one-review CLI control and one
response for accepted-stop/model-advice. Their assertions and response counts
are unchanged; new controls separately require the automatic two-repair and
two-perspective behavior. A resource blocker before a new seat preserves the
first real seat error and records the actual blocker as its cause, retaining
the genuine unavailable-review diagnostic. The original eight-call decode limit is unchanged; large new scope
fixtures issue the same paths in legal batches.

First failures are retained, not reclassified as passing: ordinary model
logging initially violated the minimal log assertion; a fixture boxed error
conversion failed compilation; the original host/config budget intersection
rejected a real rescope; new LF replacements did not match CRLF fixtures;
scope events were recorded by tools but not forwarded by the inner workspace;
new tests incorrectly expected JSON keys in human RESULT text, an Err rather
than a proper explicit unverified outcome, and only one of two actual audit
findings. Strict Clippy required a named owned HTTP callback type.

Native test/main stack overflow exposed inflated nested new futures. Compact
shared delivery/seat/model-call/finalization facades repaired the actual
configured Cargo CLI, without changing stacks, resource flags, runner
threads, dependencies, backends, Docker readiness or restore ownership.
The composed failed-full control also exceeded the unchanged 600-second
owned HTTP guard in ordinary concurrent selectors. The new source guard
redundantly recaptured Git on both sides of every model call. Original context
Git is now frozen once and rechecked before effects/completion and every
retry, with source sealing around each request and a real branch-mutation
denial control. No required check count/deadline/load flag was relaxed.

The first broad 559-case writer campaign passed 405 library, six CLI,
53 backend and two guidance cases, but the local target passed 87/93:
five new composed HTTP controls exceeded that same guard, and the old
one-response advice fixture attempted a now-authorized continuation. The
archive path had three duplicate full private-receipt traversals. It now
prepares its read-only archive plan, validates the full actual private subject
once immediately before effects, and checks the original sealed source/Git/
audit after archival. It never substitutes newly recaptured source, drops a
private verification requirement or imports positive receipts. These failing
campaigns remain retained; they are not a passing full run.

The subsequent ordinary full local campaign passes **88/93**, including every
original 60-case assertion. Five added controls still exceed the unchanged
600-second workflow guard under that runner: bounded Unicode feedback,
failed-full/rescope/perspectives, semantic repair/pass, subset changed strategy
and two-repair exhaustion. Focused real-operation controls passed during this
investigation, but those results do **not** clear the default-campaign failures. The
default matrix is not green. No deadline, failure count, runner load/thread
setting, resource flag or expectation is weakened to clear this limitation.

### Frozen writer handback: validation remains blocked

Windows GNU, stable **1.99.1** and minimum Rust **1.85.0**, Node consumer engine
**20.19.5**. Matching installed Cargo fixture compilers were selected for each
campaign. All runs are locked/offline with the ordinary runner settings,
normal system TEMP/TMP, no new dependencies and zero ignored tests.

| Gate | Actual result |
| --- | --- |
| Format, both compilers | Pass |
| Strict all-target Clippy, both compilers | Pass |
| Earlier stable all-target source freeze | 553/559 pass; six local failures retained |
| Current frozen stable full local target | 88/93 pass; five added workflow-guard failures |
| Current frozen MSRV all-target campaign | 551/559 pass: 405 library, 53 backend, six CLI, two guidance, 85 local; eight added workflow-guard failures |
| Current frozen optimized CLI/local campaign | 95/99 pass: six CLI and 89 local; four added workflow-guard failures |

The optimized failures are bounded Unicode feedback, failed-full/rescope/
perspectives, subset changed strategy and two-repair exhaustion. MSRV also
retains actual scope 3/6/12, configured Cargo CLI repair, Original Ask audit
repair and semantic fail/repair/pass failures at that same existing guard.
All original **526** cases/assertions pass in the MSRV campaign; the stable
local and optimized campaigns retain every original local/CLI assertion.
These facts do not make the expanded **559-case** default matrix green.

The current native/test/manifest freeze contains **146 files**, including
**33 added local controls**. Before/after inventories and the final handback
report identify the exact source hashes and compiler selections. No new feature
or publication follows this writing handback; independent review and resolution
of the retained validation limitation are still required.

Raw bounded campaign logs, first-failure records and before/after native/test/
manifest SHA-256 inventories are in the invocation's ignored evidence folder.
The separate file-vault storage-ownership review remains open and untouched,
even if these orchestration controls pass. No full-rewrite/cutover, Unix,
live model/GitHub/Docker, physical containment, publishing or release acceptance
is implied.

### c5bc2474 follow-up: actual prefails and guard-time evidence

V3's 146 native/test/manifest files and 32 retained evidence files were copied,
checksum-verified, into the ignored `v3-preserved` evidence directory before
writing. Original V3 inventories, assertions and failed campaigns remain.
No protected LLM, ownership/backend/Docker/restore, vault or dependency pin
changed.

The first ordinary Git selector passed 0/4 (four failures) on V3, retaining all four owned
roots. The finalizing callback control used the actual five-response docs
script and returned Verified after switching HEAD to an existing same-commit
branch. The per-tool callback control changed the second document before the
next summary finally detected drift. Plain and linked-worktree index controls
used owned `git update-index --cacheinfo` with a preexisting blob, unchanged
worktree bytes, one coder turn and one invocation request; V3 wrote the document
and returned TurnBudget. The repair returns PolicyDenied before the affected
write/completion and starts no outer continuation.

Targeted ordinary Git/index positives and negatives, healthy docs and actual
configured Cargo CLI pass **8/8 stable and 8/8 MSRV**. Both compiler format and
strict all-target Clippy gates pass. An additional owned diagnostic control
passes on both compilers: transport/accept/response counts are 1/1/1, the
transport itself succeeded, the real observer name/code is retained before
network flattening, and no dummy secret/source body enters diagnostics.

The unchanged isolated two-repair selector passes 1/1 in 109.98 seconds with
actual attempts/accepts/completed responses **15/15/15**, zero packets remaining
and the expected two-repair failed-review result. This does not clear defaults.

The single authorized ordinary default local diagnostic campaign passes
**93/100** with seven unchanged workflow-guard failures. All 100 cases and their assertions
remain active; no deadline, start point, expected count, selector/load/thread/
stack/resource flag, privacy or private verification predicate was relaxed.
The seven failed roots and owned homes are retained, not removed by fixture
drop. Each has a bounded sidecar with monotonic guard and actual-return states.

At the guard, all seven clients were pending:

| Control | Actual attempts/accepts/responses; remaining at guard | Actual return after guard |
| --- | --- | --- |
| Two semantic repairs | 8/8/8; 7 | +25.501s, Unverified, one repair |
| Subset changed strategy | 9/9/9; 6 | +46.236s, Unverified, one repair |
| Own failed-full/rescope/perspectives | 7/7/7; 9 | +144.066s, Unverified, handoff Failed |
| Bounded Unicode | 10/10/10; 0 | +100.586s, Verified |
| Semantic fail/repair/pass | 12/12/12; 0 | +112.822s, Verified |
| Original Ask audit repair | 10/10/10; 0 | +128.397s, Verified |
| Scope 3/6/12 | 8/8/8; 0 | +58.852s, Verified |

For the three partial scripts, the first actual transport errors were
respectively +13.751s, +27.247s and +130.108s after guard expiry; the listener
had already closed. None of these seven runs records an observer-origin error.
For the four fully served scripts, no transport error occurred; the real
coordinator was still completing native post-response checks when the server
guard expired. The precise awaited native operation is not identified by
these snapshots, so no load/wire/performance explanation or green claim is
substituted for that missing span evidence.

See `c5bc-actual-guard-correlation.json` for exact test names, retained owned
roots/homes, script cursors/signatures, clock origins, original first error
name/code and actual caller outcomes. V3 and new histories stay separate.
Full/release gates are not blindly repeated while this validation remains
blocked; the next bounded question is read-only native-operation span timing,
not deadline relaxation or omission of independent checks.

### Passive timing: one unchanged default replay

The independently reviewed Git/index repairs and the original 566 cases stay
intact. The passive timing module adds four unit probes, making the expected
matrix **570**; the local integration target still contains exactly **100**
cases. This diagnostic checkpoint does not run or claim a green full matrix.

Stable 1.99.1 GNU, matching fixture compiler, locked/offline dependencies and
the owned Node 20.19.5 consumer path:

```powershell
cargo +stable-x86_64-pc-windows-gnu fmt --all
cargo +stable-x86_64-pc-windows-gnu clippy --all-targets --locked --offline -- -D warnings
cargo +stable-x86_64-pc-windows-gnu test --lib execution::timing::tests --locked --offline -- --quiet
cargo +stable-x86_64-pc-windows-gnu test --test local_delivery --locked --offline -- c5bc_ configured_library_local_task_drives outer_configured_native_cli_rust_repair --quiet
cargo +stable-x86_64-pc-windows-gnu test --test local_delivery --locked --offline -- --quiet
```

Format and strict Clippy pass; passive probes pass **4/4**; the nine original
Git/index/callback/observer and healthy library/Cargo CLI controls pass **9/9**
in 261.33 seconds. The last command is the sole default timing replay:
**92 passed, 8 failed, 0 ignored, 100 total**, exit 101, 2076.11 seconds.
No selector, parallelism, request horizon, check count, native/transport/backend
deadline, 600-second guard/start point, thread, stack or resource flag changes.
No full/all-target runtime, release or new MSRV campaign follows it.

At every failed guard the client is pending. Counts below are actual
attempts/accepts/completed responses, not scripted expectations:

| Failed control | Counts of intended; remaining | Deepest sampled native await | Actual return after guard |
| --- | --- | --- | --- |
| Scope 3/6/12 | 8/8/8 of 8; 0 | owned Git, 0.304s pending | +53.529s, Verified |
| Bounded Unicode | 9/9/9 of 10; 1 | owned Git, 0.371s pending | +41.267s, Unverified |
| Configured Cargo CLI | unknown/14/14 of 14; 0 | separate-process handle unavailable | +37.911s, Verified |
| Own failed-full | 6/6/6 of 16; 10 | classification, 98.619s pending | +62.194s, Unverified |
| Original Ask audit | 8/8/8 of 10; 2 | host review observations, 8.845s pending | +28.416s, Unverified |
| Semantic repair/pass | 11/11/11 of 12; 1 | owned Git, 2.028s pending | +59.238s, Unverified |
| Subset changed strategy | 9/9/9 of 15; 6 | owned Git, 0.254s pending | +31.676s, Unverified |
| Two semantic repairs | 10/10/10 of 15; 5 | owned Git, 0.448s pending | +65.458s, Unverified |

The CLI's eventual native invocation log reports 14 actual sends, but its
at-guard send count and live internal span are unknown. For the six partial
scripts, first actual transport errors occur respectively +25.195s (Unicode),
+47.198s (failed-full), +14.185s (Ask audit), +40.888s (semantic), +17.764s
(subset) and +59.206s (two repairs) after guard expiry. All eight record no
observer-origin error. The two fully served runs have no transport failure.
The two-repair case has actually started two repairs at return; the other
partial scripts do not satisfy their expected terminal outcomes. Nothing is
converted into a pass because the listener guard expired.

The scope run gives a concrete non-overlapping critical-path partition at
native time 599.992290s, whose major components include 179 completed owned
Git awaits totaling **257.938s**
(maximum 7.337s), another Git await is active for 0.304s, preparation accounts
for 89.700s, current end-to-end evidence checks for 65.656s and host review
observations for 12.145s. Coordinator self-time is 117.292s; it is explicitly
not attributed to a more specific operation. HEAD/branch query families
contain the owned command awaits, so their inclusive durations are not added
again. Source capture/audit is 4.469s and the actual original-index checks
total 0.024s. Actual model awaits total 0.611s across eight responses.
Of the 179 Git commands, 66 HEAD and 66 branch queries account for
184.141497s inside their owned command awaits.

Across all seven library guards, completed owned Git awaits number 100–188
and consume 147.461–268.871s; their maximum individual intervals are
6.186–7.817s. They are cumulative actual `process::execute` await costs, not
a single stuck 600-second request. Failed-full classification has one
completed 130.165s inclusive/114.892s exclusive interval and a second interval
from native 501.365964s to 645.028594s. At the guard that second interval is
98.619s old; it settles successfully before the first post-guard failed send.
Protected stamp/scratch/classification suboperations are not separately
measured, so no internal cause is invented.
Process spawn/wait/output/cleanup costs inside the owned command boundary are
not separately measured; these data do not authorize changing ownership or
dropping current-state checks.

Parent IDs and child-interval unions partition each instrumented guard with
only 11–19 microseconds of boundary bookkeeping residual, zero unknown
exclusive durations and no active child truncation. Completed-record eviction
is reported; aggregate counts/totals remain retained. Capture brackets are
sampled before native-log reads. No mutex/borrow is held across an await.
Snapshots contain static labels/scalars and error name/code only, not raw
arguments, paths, prompts, tool/source/key bodies or full causes. Owned
root/home names remain only in private fixture evidence.

See the ignored outer-local evidence files `timing-default-local-once.log`,
`timing-guard-correlation.json`, `timing-hotpath-summary.json`,
`timing-campaign-metadata.json` and
`timing-diagnostic-source.json`. They retain exact failed names and roots,
script phases, active/settled parent intervals, counters and source/binary
hashes. All eight failed roots/homes remain. The 149-file native/test/manifest
freeze is byte-identical before/after the replay; all 26 protected files,
original `build.rs` and clean contracts pin remain unchanged. Original V3,
MSRV, optimized and prior 93/100 failures remain separate historical evidence.
The configured Cargo CLI is positive in the nine-target run but fails only
its server guard in this default replay despite later returning Verified;
that additional observation is retained, not explained away or retried green.

Writing is frozen for independent review. No performance fix, cached/omitted
check, new route/runtime, changed guard, storage repair, publication, full-green
or cutover claim follows these measurements.

### Query consolidation preflight: retained native refusal failure

The new selector is:

```powershell
cargo +stable-x86_64-pc-windows-gnu test --lib query_subject_original_seal_refuses_deleted_branch_masked_by_head_named_tag --locked --offline -- --nocapture
cargo +1.85.0-x86_64-pc-windows-gnu test --lib query_subject_original_seal_refuses_deleted_branch_masked_by_head_named_tag --locked --offline -- --nocapture
```

Each first execution fails **0/1**, exit 101: 4.53s stable 1.99.1 GNU and 2.87s
Rust 1.85 GNU. Both use the unchanged hardened actual Git host, original
private seal and owned writer lease. The real branch-ref inspection fails,
task/recipe/index bytes remain identical, the legacy subject still equals the
original, and `require_git` incorrectly returns Ok. Cleanup is awaited before
the expected-refusal assertion; both failed fixture roots are retained:
`.execution-native-133540-0` and `.execution-native-104156-0`.
No failure was retried green, no fresh seal was captured after drift, and no
passing coder/reviewer result or Verified delivery was claimed.

Format and strict all-target Clippy pass on both compilers. The unchanged
passive probes pass **4/4** on each. The old 570 cases/assertions and all local
HTTP scripts/100-case target remain unchanged; this adds one native regression,
making **571 expected**, with that regression red. The old nine Git/observer/
healthy targets were not rerun on this test-only checkpoint; their timing149
results remain historical proof, not newly asserted passes. No default replay
is run because the required equivalence precondition is blocked.

The only native changes are the new regression module and a test-fixture
retention flag defaulting false for existing tests. Production files, all
protected engines/vault/dependencies, original build script, current instruction
learning and contracts pin remain unchanged. The ignored outer-local evidence
contains `query-original-seal-stable-first.log`,
`query-original-seal-msrv-first.log`, `query-shape-probe.json` and
`query-consolidation-baseline.json`; timing149/V3/148 histories remain separate.
The probe also retains the first fixture-setup error: porcelain rejects the
tag name HEAD, while actual `git update-ref refs/tags/HEAD` accepts that ref.
This is an owned Git metadata control, not manual index corruption or a
storage implementation change.

See RUST's semantic-preflight block. SHA256/linked-worktree combined-query
equivalence and a new parser's malformed/missing/extra/invalid-UTF8 controls
are not claimed: no producer/parser was adopted after this counterexample.
Resolving the actual-HEAD authority decision requires an explicit next grant;
there is no cache, skipped check, broader Source/Git memoization, performance
fix, release/full campaign or default/cutover/storage acceptance.

### Authorized actual-branch repair and single-query validation

The preceding query150 block is historical. Parent review confirmed an existing
committed-task-branch authority bug and explicitly authorized its enforcement,
without a product/spec change. The old test version, native 0/1 failures,
sources and roots are preserved read-only in `query150-preserved`. Only that
new characterization was refined into an active explicit-refusal regression;
the original 570 cases/assertions are not weakened.

The current library has 421 cases and the expected total is **582**: the old
570, the refined query150 regression and 11 added actual-query/parser controls.
The local target remains exactly **100** cases with the original scripts,
guard/start point, deadlines, budgets, default parallelism, thread/stack and
resource settings.

| Serial targeted gate | Stable 1.99.1 GNU | Rust 1.85 GNU |
| --- | --- | --- |
| Format and strict all-target Clippy | pass | pass |
| Authority, valid-display, linked/hash, parser and passive probes | 16/16, 29.28s | 16/16, 26.26s |
| Original nine Git/index/callback/observer/library/Cargo CLI controls | 9/9, 200.27s | 9/9, 181.70s |

The selected unit command uses the existing runner and no scheduling override:

```powershell
cargo +stable-x86_64-pc-windows-gnu test --lib --locked --offline -- execution::tests::query_tests execution::checkpoints::query_parser_tests execution::timing::tests --quiet
cargo +stable-x86_64-pc-windows-gnu test --test local_delivery --locked --offline -- c5bc_ configured_library_local_task_drives outer_configured_native_cli_rust_repair --quiet
```

The same selectors run with `+1.85.0-x86_64-pc-windows-gnu`; fixture compilers
match each outer compiler. Dependencies are locked/offline and the existing
owned Node 20.19.5 consumer path is unchanged.

Actual Git controls cover both object formats; ordinary and linked worktrees;
the real linked index rather than the common index; detached/unborn refusals;
valid `(detached)` and HEAD branch names; colliding tag/branch and namespace-like
names; Unicode/edge-trim displays; valid coexisting HEAD tags at a distinct OID;
and original-branch deletion masked by tags/heads/remotes/other HEAD refs.
The new Git-start control asserts the real native preflight Error/
ROSTER_PROCESS_START, not an invented later-query error. The parser rejects
malformed/missing/extra records and fields, invalid UTF-8, oversized output,
wrong markers/types/namespaces and invalid SHA1/SHA256 IDs.

Two new-fixture first failures remain recorded, not removed:

- Initial **10/11**, 57.77s: linked setup used an implicit HEAD start after
  adding the HEAD-named tag. Actual Git refused that ambiguous setup before
  the selected producer. The correction supplies the already measured owned
  commit OID to worktree creation, keeping the tag and all display/index checks.
- Next **11/12**, 41.50s: the missing-image control incorrectly expected its
  explicit process-start failure after `Boundary::open`. Native preflight
  already refused it there. The correction asserts that exact earlier error.

The first logs, source inventories and owned roots
`.execution-native-132932-3` and `.execution-native-125348-2` remain.
No old assertion, packet count, guard, load, timing or resource flag changed
to repair either fixture. The earlier tag-only authority control also passed
1/1 on both compilers before physical query coalescing.

The sole subsequent default command was unchanged:

```powershell
cargo +stable-x86_64-pc-windows-gnu test --test local_delivery --locked --offline -- --quiet
```

It finishes **97 passed, 3 failed, 0 ignored, 100 total**, exit 101, 1617.63s.
No second replay or full/release campaign follows. All three clients are still
pending at their unchanged guard, with no observer-origin error:

| Failed control | Actual attempts/accepts/responses of intended; remaining | Pending boundary at guard | Actual return after guard |
| --- | --- | --- | --- |
| Own failed-full | 7/7/7 of 16; 9 | classification, 86.119857s inclusive active | +37.088s, Unverified/Failed |
| Subset changed strategy | 15/15/15 of 15; 0 | current end-to-end evidence, 10.904281s active | +27.077s, Verified |
| Two semantic repairs | 15/15/15 of 15; 0 | current end-to-end evidence, 1.390115s active | +0.882s, semantic Unverified |

Only failed-full records a transport failure: Error/null, +24.330s after guard.
Its third classification call runs from native 513.876641s to 622.129301s;
at the guard it has 77.439757 exclusive seconds, alongside 180.433526
completed exclusive classification seconds. A routine returning Ok is not
evidence that the failed full check passed. The original negative obligation
remains unresolved; the interrupted control still expects RepeatedFailure
and does not satisfy its acceptance assertions.

For the other two guards, 13/14 completed current-evidence checks total
115.802570/120.058589 exclusive seconds, with 10.904280/1.390114 active exclusive
seconds. Their full scripts and intended semantic terminal outcomes do not
make a failed workflow guard pass. All three failed root/home pairs and
bounded sidecars are retained. Known exclusive intervals partition the native
guards with 10/15/10 microseconds residual and zero unknown exclusive counts.
Protected classification/stamp suboperations remain unmeasured internally.

Physical query reduction is directly measured, without a load/OS-speed claim.
On matching completed eight-response, two-rescope Verified workflows:

| Actual counter | Timing149 | Current source |
| --- | --- | --- |
| HEAD-only/branch-only validation queries | 72 + 72 | 0 + 0 |
| Combined actual-current/commit/full-ref query | 0 | 72 |
| All owned Git commands | 191 | 119 |
| Original private Git validation checks | 37 | 37 |
| Actual original-index checks | 74 | 74 |

The actual unit counter also records two fresh validations/two owned queries,
not four or a reused positive result. Source snapshot and native index/config/
task/recipe byte checks remain active. Wall-await totals and campaign elapsed
times are retained as observations only; these are not controlled intrinsic
Git/OS/load benchmarks.

See ignored outer-local `actual-head-default-source.json`,
`actual-head-default-local-once.log`, `actual-head-default-correlation.json`,
`actual-head-matched-scope-profile.json`, `actual-head-first-failures.jsonl`
and the frozen handback. Source150, all 26 protected files, the eight current
instruction baselines, original build script and clean contracts pin are
verified. Earlier V3/148/149/150 and full/MSRV/release failures stay separate.
No default/component/cutover or separate storage acceptance is claimed;
writing stops for fresh independent patch review, not another feature.

### Deep passive proof-cost diagnostics

The reviewed actual-head150 checkpoint, its eight instruction baselines and
all three prior failure root/home timelines are preserved in
`actual-head150-preserved`. Passive instrumentation does not alter the actual
committed-ref query, original index/callback guards or any old condition.
One actual private-Stamp timing/source-refusal/privacy probe makes **583
expected cases**: the existing 582 plus one. Local remains exactly 100.

| Serial targeted gate | Stable GNU | Rust 1.85 GNU |
| --- | --- | --- |
| Format/strict all-target Clippy | pass | pass |
| Required unit controls plus new passive probe | 17/17, 35.83s | 17/17, 35.72s |
| Original nine delivery controls | 9/9, 223.82s | 9/9, 226.08s |
| Classification/e2e positive and refusal controls | 4/4, 79.08s | 4/4, 70.68s |

The backend selectors cover original full-exit preservation for preexisting,
transient and own-regression classifications; actual classification/CLI
receipts rejecting changed source; unmapped same-file failures not becoming
outside proof; and option-shaped recipe operands refusing execution. No
selection includes a live Docker/model/GitHub workload or touches the open
storage implementation.

The smallest proper diagnostic command ran first:

```powershell
cargo +stable-x86_64-pc-windows-gnu test --test local_delivery --locked --offline -- outer_failed_full_own_regression_survives_rescope_scoped_green_and_both_perspectives --quiet
```

It passes **1/1**, 291.16s, actual sends/accepts/responses **16/16/16** and no
packets remaining. The real outcome remains Unverified/RepeatedFailure,
rescope 1, perspectives 2, five seat attempts. Ten actual classifications
retain the sticky negative full obligation. They perform ten baseline
materializations/seventy blob reads and forty four-part Git signatures
(HEAD, staged-index diff, refs including HEAD and local configuration).
The complete invocation measures 295 backend Git executes/131.344s and 96
consumer executes/54.934s; source capture/image/current-plan work is separately
measured and much smaller. No guard is reached, so threshold attribution in
the earlier 100-case context cannot be inferred from this isolated timing.

That establishes the condition for the one permitted unchanged default
diagnostic replay:

```powershell
cargo +stable-x86_64-pc-windows-gnu test --test local_delivery --locked --offline -- --quiet
```

It finishes **98 passed, 2 failed, 0 ignored, 100 total**, exit 101, 1587.37s.
No source change, warming run, additional default replay, full/release gate,
packet/count/deadline/guard-start/load/thread/stack/resource modification is
used. Both clients are pending at the original 600s guard:

| Control | Actual attempts/accepts/responses; remaining | Exact pending child | Actual outcome after guard |
| --- | --- | --- | --- |
| Failed-full | 7/7/7 of 16; 9 | classify baseline → Git refs → backend Git execute, 0.006769s active | +52.995s Unverified/Failed; first transport Error/null +40.659s |
| Subset | 15/15/15 of 15; 0 | e2e current → Stamp current/capture → Git HEAD → backend Git execute, 1.014419s active | +30.494s Verified, two repairs; no transport error |

Neither run records an observer-origin error. The formerly failing two-repair
workflow completes in this replay without a guard, with fifteen actual
responses, two repairs and genuine ReviewFailed/ROSTER_REVIEW_FAILED. Its
client-return snapshot is 23.361s before that fixture's guard deadline.
That changed observation is retained, not turned into a load/OS explanation.

At failed-full's guard the accumulated leaf work is 79 backend Git executes,
**243.178753s**, and 27 consumer executes, **63.879970s**. The corresponding
Git families count HEAD/index/refs/config **10/10/9/9** completed inspections,
with refs currently pending, plus three tree reads, twenty-one blobs, three
clones and fourteen other Git operations. Its enclosing third classification
is 82.870488s old, with no single Git await that old.

At subset's guard 88 backend Git executes account for **222.241263s**.
Nineteen complete signature captures contribute nineteen each of HEAD/index/
refs/config, with the next HEAD inspection pending. Source capture's known
exclusive total is **0.639890s** and 4768 host-image-current checks total
**1.681742s**. These observations identify awaited command work, not intrinsic
Git CPU, job-provider, OS or scheduling causes inside the protected executor.

Snapshot size and interval limits remain unchanged. The command/result of an
actual await is not a semantic pass. Root parent child-interval caps produce
explicit unknown exclusives: failed-full has one completed/one active unknown,
subset three completed unknowns. Completed-ring eviction can remove a formerly
pending child's later end record; it stays unknown rather than inferred from
client return. Do not add inclusive parent totals to their child totals.
All active chains, aggregates, cap/eviction flags and clock brackets remain in
bounded private sidecars; no raw args/names/source/key bodies are added.

The completed matching scope path still records **37** original Git
validations, **74** actual-index checks, **72** current-ref queries and **119**
boundary owned Git calls. All 582 existing assertions remain in place.
Backend changes are passive only; owned/Docker/restore/LLM/vault sources,
dependencies, policy/workflows, build script and contracts pin stay unchanged.

See ignored outer-local `deep-proof-isolated-f-once.log`,
`deep-proof-isolated-f-result.json`, `deep-proof-default-local-once.log`,
`deep-proof-default-correlation.json`, `deep-proof-e-completed.json`,
`deep-proof-matched-scope-profile.json` and the handback. Prior failure roots/
homes are preserved, as are the two new guard root/home pairs. The proposed
same-capture HEAD/ref coalescing is not implemented. No Source/image cache,
proof/check skip, status-JSON authority substitute, storage fix, green default
or component/cutover acceptance follows this diagnostic freeze.

### Current signature and validation-repair candidate

The fresh-signature coalescing patch was reviewed independently before this
repair phase. It retains the exact four logical byte hashes while reading
index, fresh required refs/derived HEAD, then local config in three actual
owned Git calls. No raw refs/config UTF-8 conversion, prior Stamp reuse,
source/current/index/callback skip or changed consumer command is introduced.
Six pure byte cases and five real-Git cases cover exact/duplicate/malformed/
missing/oversized HEAD, SHA1/SHA256, linked/detached roots, HEAD collisions,
actual arbitrary ref/config bytes, drift and cancellation. The old passive
probe's directly coupled physical expectations are truthfully 12 to 9, not
fake process events; all its authority/refusal/privacy conditions remain.

Its pre-repair default was 93/100 with five guard failures and two
ROSTER_PROCESS_START failures. Parent archived that source151/evidence and old
fixtures by rename. This phase only reads that archive. No archive/old root
or sibling HOME content is modified.

Start investigations, in order:

- Unchanged exact two start selectors: **2/2**, 66.80s. This is not historical
  cause proof or evidence of a transient/load issue.
- Initial bounded cwd/cancel diagnostic controls plus unchanged start
  assertions: **4/4**, 59.57s. New tests were then moved to library coverage so
  default local remains exactly 100.
- All seven affected existing selectors with passive facts: **7/7**, 311.74s.
  Original STALE_SOURCE/submodule and continuation acceptance assertions pass.
- Actual cwd diagnostics observe OS267/NotADirectory with missing/file cwd,
  passed existing image/token checks and no child PID. Precancellation keeps
  ROSTER_CANCELLED and does not fabricate spawn or stale evidence.

The original two historical errors lack their OS source and remain causally
unproven. Current errors now retain bounded failure-only scalar facts and both
start controls retain returned-error fixtures; there is no ProcessStart-to-
Stale catch, fabricated source proof or success-shaped fallback.

New start spans measured 61 real start Git calls per complete affected run,
including two separate eleven-path ignore boundaries. One native library
control proves all eleven legacy per-path checks, the one-call complete
coverage result and refusal when Git returns success for only a subset.
The production coordinator now uses two calls rather than 22 for those same
eleven paths per boundary; matching complete start counts are **61 to 41**.
No stdin/transport/engine extension, classification or test count reduction,
ignore/policy write, cached observation or guard change is used.

| Serial repaired gate | Stable GNU | Rust 1.85 GNU |
| --- | --- | --- |
| Format/strict all-target Clippy | pass | pass |
| Unit controls, including original 28 plus new three | 31/31, 68.85s | 31/31, 62.31s |
| Original nine delivery plus two start cases | 11/11, 222.44s | 11/11, 219.24s |
| Classification/current-evidence positive/negative proof | 4/4, 53.45s | 4/4, 72.56s |

The five affected workflow selectors then pass **5/5**, 316.10s, with original
actual sends/accepts/responses 16, 15, 15, 12 and 10 and their real
RepeatedFailure/ReviewFailed/Verified outcomes. Compiler/consumer paths are
the same private stable/MSRV toolchains and owned Node20, locked/offline.

Only then did the one authorized unchanged command run:

```powershell
cargo +stable-x86_64-pc-windows-gnu test --test local_delivery --locked --offline -- --quiet
```

Current result: **96 passed, 4 failed, 0 ignored, 100 total**, exit101,
1697.61s. No second default/full/release campaign follows. Original start
cases pass; their old OS cause is not inferred from that changed observation.

| Remaining guard | Actual attempts/accepts/responses; remaining | Deepest pending actual operation | Actual return after guard |
| --- | --- | --- | --- |
| Scope 3/6/12 | 8/8/8 of8; 0 | current reviewed-subject check → boundary current-ref Git, 0.819090s active | +41.606s Verified |
| Bounded Unicode | 10/10/10 of10; 0 | current e2e → Stamp → refs Git execute, 0.746147s active | +66.719s Verified |
| Own failed-full | 7/7/7 of16; 9 | classification baseline → tree Git execute, 5.243260s active | +103.675s Unverified/Failed |
| Subset strategy | 15/15/15 of15; 0 | current backend Git execute, 1.352883s active | +0.476s Verified |

Failed-full first records actual transport Error/null +88.948s after the
guard. The others have no transport error; none records observer-origin
failure. Root parent exclusive unknowns and record evictions remain explicit.
Do not replace the guards by the eventual outcomes or infer intrinsic
Git/OS/load latency from these campaigns.

At guard start Git executes number 37 on the first three paths and 41 on the
subset path, totaling **176.083–207.675s** and **129.943s** respectively.
Three `Repository::inspect` calls still each query actual top/common/git-dir
locations separately. The next surgical proposal, not implemented, is one
fresh existing Git producer for those three physical reads, preserving every
identity/source/index/config/ref comparison. Broader blob batching/native
stdin transport remains outside this freeze.

All original 594 assertions remain; new controls are library-only, making
**597 expected cases** (436 library/53 backend/100 local/6 CLI/2 guidance).
The repair patch subsequently received a clear independent source review,
not historical OS-cause or default-green acceptance. No full597 stable/MSRV or current release/CI
acceptance is claimed; storage ownership remains a separate open blocker.

Fresh output is the new ignored `.roster/outer-local-milestone`. After PID136620
exited with no observed direct children, ten current-PID returned/retained
containers were attested and renamed, byte/hash-preserved, into
`validation-repair-retained-fixtures` there. Four are failed guard cases; six
are retained expected-negative fixtures. No evidence was deleted and every
original root/home mapping is retained in
`validation-repair-fixture-preservation.json`. Associated sibling homes and
the older external archive are untouched; no top-level native fixture clutter
from these checks remains.

See `validation-repair-default-source.json`,
`validation-repair-default-local-once.log`,
`validation-repair-default-correlation.json`,
`validation-repair-next-proposal.json` and the frozen handback. Source hashes,
new provenance, exact native prefails/facts/gates, protected files, instructions,
build script and SDK pin are recorded for review. No publication or whole
rewrite/cutover acceptance follows this handback.

### Repository path-read coalescing: targeted freeze

The next bounded patch replaces only the three location reads inside each
fresh `Repository::inspect` with one existing owned Git observation. It keeps
the original pre-query native identities, all three comparisons, relative
resolution, and every subsequent source/config/HEAD/branch/index/worktree
check. A strict 16 MiB UTF-8 three-record parser preserves per-record trimming.
Known expected OS paths with LF select the original three calls before any
query; malformed/error output never triggers a compatibility retry.

| Serial path-query gate | Stable GNU | Rust 1.85 GNU |
| --- | --- | --- |
| Format/strict all-target Clippy | pass | pass |
| Original 31 plus nine new unit controls | 40/40, 84.11s | 40/40, 87.71s |
| Original nine delivery plus two start controls | 11/11, 171.86s | 11/11, 194.08s |
| Classification/current-evidence/baseline proof | 4/4, 77.59s | 4/4, 79.54s |

The initial new selector was **7/9**, 30.03s; both roots, source and log remain
preserved. Windows refused symlink fixture creation with actual OS1314, and
SHA256 full inspection encountered the existing `extensions.*` config denial.
Only the new fixture expectations changed: native hard-linked marker refusal
is proved before any Git submission; SHA256 raw-path parity and the existing
config denial are both required. The corrected new selector is **9/9**, 34.59s.
The Unix-only actual symlink control is not executed on this host. No Windows
symlink privilege change or actual Unix LF-path proof is claimed.

Real owned queries compare the old three raw outputs with one actual combined
output byte-for-byte, then repeat the production helper twice, checking all
three identities and unchanged index/config/TASK/RECIPE bytes. Recorded count
assertions require three old physical reads versus one per fresh combined
observation, with no old-path commands in the combined observations. Current
source/config/staged-index/ref changes still return `ROSTER_STALE_SOURCE`.
No elapsed-latency improvement is inferred from different campaign timings.

The Windows GNU candidate has **606 expected cases** (445 library/53 backend/
100 local/6 CLI/2 guidance); all original 597 conditions and the original
600-second guard, test parallelism and HTTP-script assertions remain. Native
files remain 151. No new default100/full606/release campaign is run. Previous
default96/100 remains historical evidence, not acceptance of this source.
This path-query patch was subsequently cleared by independent source review; owned stdin/blob
transport, whole-file storage and broader release/cutover acceptance remain
outside this patch.

### Finite-input Stage A: native ownership counterproof, not acceptance

The preceding path-query source was independently cleared; the parent also
ran its nine path controls, **9/9**, 31.91s, against the matching frozen source.
The next finite-input attempt has an explicit API-gap stop rather than a
passing implementation.

| Bounded diagnostic on the prototype | Stable GNU | Rust 1.85 GNU |
| --- | --- | --- |
| Initial binary/EOF and pre-spawn input-bound TDD | 1/2, 1.87s | not run |
| First binary/duplex/pending-writer small control | 3/4, 4.93s | not run |
| Direct leader observation plus required busy-writer ownership | 1/2, 2.14s | 1/2, 3.62s |
| Prototype format/strict all-target Clippy | format run | pass |

Both final native probes report only bounded scalar facts:
`idle_handle_preserved=true`, `full_256KiB_write_accepted=true`,
`flush_pending=true`, `busy_handle_duplicated=true`,
`actual_job_cleanup=complete`. The direct original-process-handle leader wait
passes; the desired busy-writer ownership assertion fails. No input bytes,
handle values or command bodies are printed.

Exact installed Tokio provider code confirms the mechanism: Windows
`ChildStdio` uses an `Arc<StdFile>` and `Blocking<ArcFile>`. `poll_write`
acknowledges the queued buffer before its blocking task completes; flush waits
for that task, shutdown is a no-op, and safe handle transfer duplicates a
handle if dropping the I/O wrapper cannot remove the task's reference.
Closing the public object is not proof that the actual writer has been
closed/joined before owned-job shutdown. No speculative pipe replacement or
raw-handle cancellation workaround was implemented.

The complete 153-file prototype, thirteen proposed controls, all first native
failure versions/logs and failed roots are retained in ignored forensic
evidence. The other input-cancellation/drop/overflow/uncertain-cleanup controls
are not claimed passed. The active native source was restored to the exact
reviewed 151-file, **606 expected case** baseline; the new prototype and tests
are not silently included as a completed primitive or removed from its record.

| Restored-baseline preservation gate | Stable GNU | Rust 1.85 GNU |
| --- | --- | --- |
| Format/strict all-target Clippy | pass | pass |
| Existing NoInput/submission/process/cancellation/death controls | 22/22, 2.52s | 22/22, 7.73s |
| Existing CLI controls | 6/6, 1.68s | 6/6, 1.15s |
| Actual dropped-preplan future retains writer quarantine | 1/1, 2.87s | 1/1, 2.74s |

These gates prove preservation after reverting only this attempt; they do not
accept finite input. Stage A is blocked on a separately reviewed
cancellation-owned pipe design. Stage B, the previous default four guards,
full/release/platform/CI gates and the separate storage ownership issue remain
open. No default100, full606, release or live workload was run.

### Stage A retained-and-joined writer: bounded native freeze

After the independently confirmed busy-drop counterproof, Ray explicitly
approved retaining the original stdin/worker through abort and joining it
concurrently with owned job shutdown. The previous failed pre-kill closure
test remains frozen and failed; it is not renamed, weakened or retroactively
accepted. No named-pipe replacement was implemented.

The new distinct after-kill native control starts with all 256 KiB accepted,
flush still pending and no disposed handle, cancels at readiness, then requires
the original reason, actual job death, terminal flush (including broken pipe),
**original rather than duplicated handle disposal**, known cleanup and actual
lease release. Deadline/overflow controls require the same release ordering.
Full-duplex stdout/stderr exceed pipe buffers before input is read. Max/binary/
NUL/empty input, early leader/exit0, descendant-held pipes and whole-job waiting
are tested without changing budgets or runtime resources.

Four real-quarantine controls use the existing `PendingOwner` via a tiny
Windows-test-only factory: pending-input Drop without a second cancellation
poll, unpolled future, post-EOF Drop while a descendant remains, and an actual
late terminal join after abandonment. Real `RootLease::close` remains refused
after those abandoned outcomes; later physical completion is not authority.
Typed error/cleanup-deadline boundary tests are not a claim of inducing an
actual kernel shutdown fault.

| Exact final v2 source, serial bounded gate | Stable GNU | Rust 1.85 GNU |
| --- | --- | --- |
| Format, standalone Rust fixture format, strict all-target Clippy | pass | pass |
| Sixteen new plus original 22 NoInput/death/submission/cancel controls | 38/38, 24.57s | 38/38, 26.34s |
| Original CLI controls | 6/6, 2.38s | 6/6, 1.17s |
| Existing actual dropped-preplan writer-quarantine control | 1/1, 4.27s | 1/1, 2.64s |

The first compile-scope error, distinct native TDD **1/2** and source/root are
preserved. Implementing terminal join then passed the small **5/5**, 4.35s,
and first **15/15**, 20.65s. A test-only factory lifetime triggered the first
MSRV strict lint after the initial stable gate; its source/log are retained.
Only that lifetime was elided, then the exact final v2 source passed both
compiler campaigns above. There are zero ignored cases. The revised primitive
never treats write acceptance or shutdown-no-op as a worker join.

The source has **153 native/test/manifest files and 622 expected Windows GNU
cases** (461 library/53 backend/100 local/6 CLI/2 guidance), preserving all old
606 conditions. Original NoInput command setup, null stdin, private submission,
whole-job provider, flags and limits remain. No source/proof/receipt cache,
model stdin option, new dependency, unsafe code or executor was added.
The current core is frozen for c5bc review; no Stage B Owner/Git/materializer,
default100, full622 or release gate is run. Old default failures, human CI,
other-platform and separate native vault-storage acceptance remain open.

### Stage B immutable blobs: targeted proof and one red default

Stage A's accepted owned core/tests are byte-identical. The baseline records
the historical Stage A handback hash and the parent's metadata-only corrected
attribution hash separately; no metadata fork grants human acceptance.
New pure frame/chunk and real-Git controls cover main/linked/detached,
SHA1/SHA256, raw binary/zero/max bodies, a valid 128 MiB baseline, managed
oversize skips, modes, duplicate OIDs/paths, package refusal before clone,
approved submodules/depth, current source/index/ref/config/owner, actual owned
overflow/error/cancellation/task deadline, partial writes and quarantine.
The task-deadline control uses the caller token; Stage A's existing native
command-timeout controls remain unchanged.

The first broad selector was **12/13**, 78.21s: a new maximum-size fixture
failed before materialization because template-disabled init had no
`.git/info` directory for its ignore setup. Its exact source/log/root are
retained. A fixture-only mkdir correction passes actual 128 MiB **1/1**,
19.73s. Separate whole-chunk error/cancel/unknown controls pass **2/2**, 9.00s.

The first stable unit/CLI gates passed, but configured Cargo CLI failed with
an actual main-thread stack overflow after five requests. Both failed roots,
HOMEs, source and logs are retained. Measured new batch-future pinning
(10,336 to 192 bytes) alone left the CLI failure. Compacting the actual
existing Owner Git await boundaries reduced its frame 10,560 to 368 bytes
and materializer 11,232 to 1,040; the exact CLI passes **1/1**, 90.09s, with
its original fourteen requests. No stack, scheduler, thread or timeout flag
was changed, and the accepted lower core was not edited.

| Exact final v2 source, serial gate | Stable GNU | Rust 1.85 GNU |
| --- | --- | --- |
| Format/strict all-target Clippy | pass | pass |
| Sixteen new plus all broader Stage A59 units | 75/75, 106.68s | 75/75, 97.06s |
| Existing CLI | 6/6, 0.31s | 6/6, 1.20s |
| Git/index/callback/configured positives, start and quarantine | 12/12, 167.43s | 12/12, 190.63s |
| Current-evidence/classification/baseline proof | 4/4, 70.81s | 4/4, 82.25s |
| Actual failed-full and current review workflows | F1/1, 156.40s; B/E2/2, 93.26s | F/B/E3/3, 215.49s |

Complete F still sends/accepts/serves sixteen requests and ends in required
RepeatedFailure, with one rescope and both perspectives. Its ten baseline
materializations request seventy logical blobs in ten actual batches, rather
than seventy physical per-blob calls. Backend Git processes are **195 versus
255** in the matched historical signature151 trace; twenty Stamp captures,
forty fresh signatures (all four logical components), twenty setup and twenty
consumer test runs remain. Git validation/index counts for this F trace are
40/80; do not conflate them with the separate scope workflow's 37/74.

Only after those targets passed did the original unchanged default command
run once: **98 passed, 2 failed, 0 ignored, 100 total**, exit101, **1342.41s**.
It used the same guard/start/parallelism/stack/resource/HTTP-script settings.

| Failed guard | Actual requests/responses; remaining | Exact pending awaited operation at native guard | Actual later result |
| --- | --- | --- | --- |
| Failed-full F | 8/8 of16; 8 | classification → fourth baseline → `update-ref --no-deref HEAD` owned Git, 0.161926s active | +107.451s Unverified/Failed; first transport error +94.179s after guard |
| Subset review | 15/15 of15; 0 | reviewer current evidence → e2e current → Stamp → cached-index diff owned Git, 3.403573s active | +17.784s Verified; no transport error |

F's native clock is 599.991986s at capture; its fourth baseline has completed
tree and clone and has not yet completed `update-ref`. Three batches/21
logical blobs and three baselines completed; 65 backend Git waits total
240.012947s, including thirteen config reads totaling 71.775966s. Separate
completed boundary Git/start Git/command waits total 128.628317/92.278101/
118.175252s. These leaf families are not added to their inclusive parent
classification/Stamp/base times. Subset's twenty completed fresh signatures
have 20 index/refs/config reads each; 72 backend Git waits total 204.326095s.
The active current cached-index read is in addition to those completed reads.
Dropped records and unknown parent exclusives remain explicit in evidence.
No intrinsic OS/load/network cause or cross-campaign wall-speed gain is claimed.

The source remains frozen at **155 native/test/manifest files, 638 expected
Windows cases** (477 library/53 backend/100 local/6 CLI/2 guidance).
Both guard failures and every first failure are preserved; no second default,
full638, release, Stage C feature or storage work follows. Fresh independent
source review, current default acceptance, human CI and platform gates remain.

## Evidence and missing coverage

Port the original expected outputs, wire bytes, error cases and forbidden side
effects into native tests without weakening them. A removed or unported check
is not a pass. Zero matches, skips, cancellations, stubs and unavailable
capabilities are not successful acceptance evidence.

Historical Node/differential counts in previous migration checkpoints are not
current Rust test counts. Passing component tests does not establish full
delivery-loop equivalence, Windows ACLs, Unix descendant containment,
cross-platform binaries or the frozen live reliability campaign.

Report the command, compiler/platform, exit status, actual test counts and
remaining gaps. Keep policy/workflow/release changes human-owned and leave real
user state and configured endpoints untouched.
