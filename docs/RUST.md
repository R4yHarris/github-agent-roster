# Native Rust runtime

Ray approved the full rewrite and explicitly directed this feature branch to
remove the Node implementation and oracle-preservation changes. Main and Git
history already preserve that code. There is no copied legacy tree, frozen
reference checkout, Node application entry point or runtime fallback here.
Product trace: FEATURE_SPEC 5.1-5.8, especially 5.4 and 5.5; sections 3, 7 and 8
remain binding. See [the current prompt](../prompts/04-rust-migration.md).

## Structure and current behavior

One Cargo package supplies the [library](../rust/lib.rs) and native
[`roster` executable](../rust/main.rs), Rust 2024 with minimum Rust 1.85.
Application unsafe code is forbidden. New code has no comments beyond required
legal notices. Tokio uses one current-thread runtime for asynchronous invocations.

The library contains:

- Restricted catalogs, quoted arguments and recipe parsing/file validation.
- Lossless UTF-16 Text/JSON, JavaScript-compatible numbers and persisted bytes.
- Configuration, fleet and capability validation, identity and request metadata.
- Owned HTTP/TLS/SSE transport, retries, request limits, usage and redaction.
- Builtin seat chat, required engineering context and invocation-shared
  per-profile request admission, integrated with native planning and critics.
- Actual bounded coder and fresh read-only reviewer library loops over that
  chat, shared admission, strict tools and current private execution receipts.
- Cancellation, deadlines, roots, atomic files, death-proof locks, provenance
  and copy-only state migration.
- Task intake, planning artifacts, grounding, critique, memory and skills.
- Guarded tools, required context, scoped verification, checkpoints, frozen
  execution attempts, evidence, finalization and versioned acceptance state.
- Owned base-revision scratch, Node/Cargo counterfactuals and failure
  classifications, committed lifecycle hooks and native read-only acceptance.
- Private artifact archives, explicitly approved transactional green-candidate
  restoration and optional local-image Docker independent verification.
- A native version-one encrypted file vault, OS-bound ownership/permissions
  and selected-only endpoint credentials in the actual local run path.
- Ask drafting/submission, clarification checkpoints, scoped repository
  questions, side questions (`/btw`), bounded state/provenance cleanup,
  benchmark/debug-log/repo-map/run-log diagnostics, publication/trailer and
  wave/attempt helpers, hardware/routing/discovery/remediation helpers, shell/
  onboard helpers and paired Eve pilot evaluation.

The public CLI supports the interactive shell (`roster`, `roster shell`),
process-only debug JSONL logging (`roster --debug [COMMAND]`), interactive
onboarding (`roster onboard [--discover]`), help, recipe validation, catalog
parsing, argument splitting, offline initialization/diagnostics (`init`,
`doctor`, `doctor --copilot`, `bench`), Ask and clarification checkpoints
(`ask`, `ask --pending|--cancel|--resume` with concurrent Git privacy checks
and single-pass path validation), read-only scoped repository questions
(`question`), durable provenance history queries (`history list|show`),
bounded state/provenance cleanup (`clean`), encrypted file-vault operations
(`vault set|list|get` with post-publication handle-identity verification),
analytics and recurring failure proposals (`stats [--delivery]`,
`learn --recurring`, `recommend`, `eval`), and the local TASK/Ask delivery
coordinator, including fresh App issue intake and the fixed-profile outer
continuations below. GitHub publication/resume and automatic fleet
recovery/routing are not integrated at the top-level CLI. Unsupported commands
return an explicit error rather than invoke the old runtime or claim success.

## Native read-only analytics checkpoint

FEATURE_SPEC **5.6**, **5.2**, **5.7** and **5.8** now have native learning
validation, Git/local joins, distinct-sample aggregation, known-only medians,
advisory recommendations, actual seat/model difficulty ceilings and scoped
routing-evidence summaries. The planner reuses the same recommendation core.
No history is automatically promoted into model context or curated memory.

The real CLI paths are:

```text
roster stats [--delivery] [--json] [--ref REVISION_OR_RANGE] [--evals PATH]
roster learn --recurring
roster history list [--format json|text|summary] [--store DIR] [--repo HASH] [--issue N] [--seat NAME] [--model MODEL] [--outcome VALUE] [--since TIME] [--until TIME]
roster history show <session-or-run-or-record-id> [--format json|text|summary] [--store DIR]
roster recommend --task-class feat|fix|docs|test [--difficulty 1-5] [--json]
```

They require an absolute `ROSTER_GIT` and use the existing hardened,
cancellable owned Git transport, not Node or an SDK exporter. Ref endpoints
become exact commit IDs before logging. Inputs are bounded and read-only;
malformed/ambiguous evidence, provider errors, unsafe files and identity drift
fail without partial output or storage mutation. Explicit refs exclude local
unpublished samples. Explicit legacy eval files override matching SHAs only.
The canonical repository's shared layout is used from subdirectories and
linked worktrees; issue scratch and foreign repositories are not history.

`learning` exposes parsing, joining, summary, recommendation, ceiling and
evidence functions. `metrics::load` / `load_with_store` return `Analytics`,
whose raw rows remain private. Existing provenance can be read only from an
explicit identity-scoped machine root or injected same-identity store.
No default user-root discovery, new durable writer, state migration, database,
policy grant or automatic human evaluation is introduced.

CLI recommendations are coder advice with three human samples, sufficient
median difficulty and an earned ceiling, not a fleet/profile assignment.
`roster stats --delivery [--json]` groups delivery evidence by served model,
declared hardware and seat while preserving individual local attempt models
and usage across published commit links. `roster learn --recurring` writes
human-reviewable draft proposals in `.roster/proposals` without modifying
active rules, and `roster stats` (text mode) lists open proposals.
`roster history list` and `roster history show` read bounded durable
provenance records in `text`, `json` and `summary` formats without mutating
or repairing the store.
Automatic fleet/prior routing and the legacy profile/context/hardware suffix
are not implemented by this checkpoint. Automatic run recording,
onboarding, shell, existing-issue recovery and publication remain explicitly
unavailable.

ASCII en-US collation and UTF-16/table/number contracts are covered natively.
Non-ASCII locale ordering is refused as unproven rather than byte-sorted.
Windows GNU checks do not prove MSVC, POSIX containment, filesystem ACL
providers, complete legacy coverage or FEATURE_SPEC section 8 acceptance.
See [METRICS](METRICS.md#native-stats-and-recommendations) and
[the focused tests](TESTING.md#native-analytics-controls).

On Windows GNU, the checkpoint's **56 new tests** pass on stable Rust
**1.99.1** and Rust **1.85.0**: 40 library cases and 16 real CLI/acceptance
cases. Twenty preserved affected controls also pass on each compiler.
Format checking and strict all-target Clippy pass on both; discovery reports
**698 tests**, not 698 executed passes. Stable optimized controls pass all
16 analytics cases and all six preserved CLI cases.
The unchanged recipe-file CLI fixture still owns and removes its nonce-named
temporary root; its source/assertions were not weakened. New analytics
fixtures use only their recorded repository-owned roots.

Four independent review findings were reproduced and repaired: bounded
join expansion, human-ledger chronology, SHA/session correction isolation
and secret-free fallback configuration errors. Joined compact-JSONL bytes
are checked incrementally at 16 MiB while retaining the existing 32 MiB
combined input allowance. Human decisions retain real source ordinals;
missing timestamps do not silently substitute Git/run order.

The pre-existing 642-test baseline is not a current full-green claim:
its last default local-delivery run passed 93/100 with seven existing
600-second guard failures. Those guards, assertions and runtime/performance
policies are unchanged by analytics; the default 100/full suite is not
replayed as an analytics gate. Storage-ownership review and native CI/release
remain separate open gates. Component checks and review do not establish
full migration or cross-platform acceptance.

## Native human evaluation

FEATURE_SPEC **5.6**, **5.7** and **5.8** now have an explicit native human
writer: `roster eval TARGET accept|reject|rework 1-5 y|n --local|--post`, with optional
`--minutes` and `--comment`. This does not evaluate a seat automatically or
authorize policy. Declared App/agent invocations and
read-only seat acceptance refuse it before execution.

The writer preserves the existing shared-state resolver, JSONL history,
unknown measurements and actual source attribution. Full SHA1/SHA256 and
commit abbreviations are supported; ambiguous session metadata requires a
specific SHA or attempt. Local/Git evidence precedes explicit same-identity
durable lookup, with exact source references and no requested-model or
starting-commit substitution. Comments are redacted data. Existing locks,
bounded checked reads, atomic replacement and explicit cleanup are reused;
uncooperative vault storage ownership is not solved or weakened by this path.
Optional posting uses an explicitly approved GitHub CLI and pinned explicit
human credential, verifies its actual human principal and one exact-SHA PR
association, and sends only compact `AI-Eval`/`Minutes` after local saving.
No private comment is sent, no PR is guessed, and no uncertain write is
retried. Origin drift, cancellation and unknown child cleanup fail explicitly;
dropped submissions retain the lock. `record_local` remains strictly
network-free even when passed a posting decision.
See [human evaluation](LEARNING.md#native-human-evaluation).

## Native offline issue observations

FEATURE_SPEC **5.7** and **5.8** now have
`roster status --issue N --offline [--json]`. This path uses an absolute approved
`ROSTER_GIT` and the native registered-worktree inventory. It reads the selected
base issue worktree's schema-one session log, falling back to the canonical
repository only when that log is missing. Ambiguous issue ownership, foreign
common directories, linked/nonregular/hardlinked/unreadable files and corrupt
complete records fail explicitly without partial output.

The reader preserves the native writer's exact 2 MiB bound, including a single
large record and more than 10,000 small records. It validates schema, sequence
and metadata labels in complete UTF-8 records; an active unfinished trailing
record is explicitly ignored. Output projects only issue, safe seat/event/state
labels and complete-record metadata, never raw detail bodies, model text or
credentials. Missing observations stay unknown. Neither a recorded `pass` label
nor the existence of a log proves worker liveness, current verification,
publication or human acceptance.

The real native CLI and read-only acceptance path are connected; acceptance
adds `--offline` before implementation checking and preserves the argument cap.
Offline status does not load endpoint configuration, credentials, default user
history or a GitHub client and performs no filesystem mutation. Online issue
observations are implemented separately below. Automatic issue inference,
retry/winner selection and the broader cached issue, artifacts and measured-run
display are still unavailable. This is not complete legacy
status parity or an interactive status rail.
See [the native controls](TESTING.md#native-offline-status-controls).

## Native App-scoped issue observations

FEATURE_SPEC **5.1**, **5.7** and **5.8** now have
`roster status --issue N [--json]` and the native
[`github::read_issue`](../rust/github.rs) API. The only Node adapter calls the
required contracts SDK's public origin/authentication exports; it is not a
Roster worker, delivery loop or fallback. GitHub HTTP, issue validation,
cancellation, token revocation and status composition remain Rust, using the
existing HTTP transport without redirects, proxies or retries.

The adapter requires complete App credentials and absolute approved Git/Node20
images, with the clean dependency sealed at the installation's reviewed
gitlink. It does not import the SDK as its CLI entry point or forward ambient
human tokens. The App key must be a bounded unlinked regular file outside both
repositories. Its actual SDK-side handle read is capped at 64 KiB, not merely a
preflight metadata check; the buffer is cleared. The existing SDK token baseline
includes contents/PR write permissions; this path requests only additional
issues read and performs no task, issue, PR or policy write.

Only an exact repository/number issue is returned; PR-shaped, foreign,
incomplete and oversized responses fail. Completed reads require acknowledged
token revocation. Cancellation preserves its first reason while bounded
revocation still runs; an unacknowledged cleanup never becomes a successful
observation. Dropping an unfinished read provides no issue or task authority
and is not proof of remote token revocation. No writer is acquired or released
by this read-only intake path.

An observed issue retains the original repository identities, approved Git
provider and reviewed SDK seal. `require_current` rechecks them across later
awaits; online status revalidates after its local-log read before returning.
Output excludes the issue body and token, distinguishes local/remote facts,
and explicitly leaves PR state unknown because no PR query ran.
`status::observe` remains strictly local and refuses online options; read-only
seat acceptance always adds `--offline`.

Fresh native GitHub task delivery is described below; publication/resume and
the rest of status parity remain unported. Tests use an owned synthetic SDK interface, a dummy non-PEM
key and controlled native HTTP responses, not real credentials or GitHub calls.
This does not prove live App installation permissions, platform release,
worker cleanup or FEATURE_SPEC section 8 acceptance.
See [the controls](TESTING.md#native-app-issue-observation-controls).

## Native fresh issue delivery

FEATURE_SPEC **5.3**, **5.4**, **5.5** and **5.7** now have
`roster run --issue N --runtime builtin`, with the existing base/fetch/skip-review
controls. Complete App configuration is required before private model settings
are loaded. The App observation must identify an open issue; human credentials
never replace it. The shared strict issue-number parser refuses aliases,
fractional/unsafe values and leading zeroes consistently with status.

`delivery::resolve_issue` captures the original calling repository before its
metadata awaits, then returns an opaque immutable intake. Delivery reuses that
original source/branch/index seal rather than accepting a fresh post-await
snapshot, refuses a different calling worktree and revalidates before effects.
The shared native seal checks actual primary and split/shared index bytes and
file identity, not just status or a content diff. Same-commit branch switches
and index-only changes cannot authorize a new task through a cloned intake.
The whole CLI task deadline covers intake and all later seats. Authentication
HTTP does not consume model request admission or become model metrics.

Shared native preparation creates `agents/N`, task `issue-N`, session
`roster-N-coder` and one fresh issue worktree. Existing paths and known
canonical, legacy, attempt or remote-tracking claims are preserved and refused;
this is not resume/adoption or proof that an explicitly unfetched remote is
current. The existing prepare lock, exact approved submodule materialization
and one-writer root lease remain in use. No Kanban store or second worker
runtime was added.

Planning, guarded coding and fresh read-only review use the same process,
worktree and private completion receipts as local delivery. `intake::parse_issue_body`
validates non-empty issue bodies and optional `## Task metadata` (`task_class`,
`difficulty`, `estimate_min`) without pre-concatenating `title` into
`assignment.ask`; classification and planning receive `Some(issue.title())`
and parsed metadata explicitly, the recipe retains `issue:N`, and every
post-intake origin/SDK re-check runs on the invocation's owned `start::Git`
through completion before the typed outcome stamps the validated issue URL.
`Repository::inspect` seals primary and shared indexes before observing `HEAD`,
`status` and `source_digest`. A stub remains a no-model, no-code, no-test
result. No issue status comment/label, close, PR, publication, automatic route
or human evaluation is manufactured.

Owned controls exercise a real issue branch/worktree and both stub and
configured seven-response planning/critic/coder/self-review/fresh-review paths.
The configured path makes an actual owned guide edit, passes the existing
private gates and records distinct actual response models with reported zero
usage. Three independent review findings (issue title/body classification and
metadata parsing, owned Git tracking and completion origin re-verification, and
pre-observation index sealing) were reproduced and repaired; **33 targeted
cases** pass on both stable Rust **1.99.1** and Rust **1.85.0** GNU (15 library
cases, one analytics CLI case, 10 start/checkpoint cases and seven CLI cases),
plus optimized `--release` controls. App and model transports are synthetic;
no live App/model or harness acceptance campaign is implied.
See [the controls](TESTING.md#native-fresh-issue-delivery-controls).

## Native offline fleet listing

FEATURE_SPEC **5.2** and **5.7** now have `roster fleet list`. The synchronous
native leaf reuses canonical shared configuration scope and the checked private
settings reader, not an executor, Git process, endpoint client or new catalog
store. Missing catalog and explicit empty catalog retain the existing empty
message; configured profiles retain their original tabular field order and
unknown onboard-default capacity. No profile, default, route, credential file
or repository history is changed.

Catalogs retain the exact 64 KiB bound and must be stable single-link regular
files in unlinked checked directories. Malformed input, sharing denial and
unsafe files fail without partial tables or private provider details.
Single-line metadata refuses terminal control characters and Unicode line
separators. Lossless internal UTF-16 remains intact; unencodable console output
fails rather than silently replacing data. Known configured credential values
are redacted; notes and credential declarations are not listed.

CLI parsing and real read-only acceptance both recognize only `fleet list`;
discovery, probing, warming, mutation, onboarding and automatic routing are
still unavailable. Listing configured metadata is not endpoint discovery,
measured capacity or response-backed run evidence.
See [fleet scope](FLEET.md#native-scope) and
[the native controls](TESTING.md#native-offline-fleet-listing-controls).

## Native verification

Use Cargo formatting, strict all-target Clippy and native unit/integration
tests as described in [TESTING](TESTING.md). Port expected behavior and negative
controls from main/Git history into Rust. Removing an old test is not evidence
that its acceptance case has been implemented or passed.

The native CLI tests execute the compiled binary with an empty `PATH`; they
cover exact JSON output, Unicode/quotes, real recipe files, invalid commands,
malformed UTF-8 and the exact 4 MiB stdin limit. State tests pin version-one
marker/identity bytes and private settings replacement directly, without
loading an original JavaScript implementation.

Native execution tests may launch Node 20 against owned JavaScript consumer
fixtures through `ROSTER_TEST_NODE`. This is support for the task's language,
not a Node Roster library. The independent contracts SDK remains a Node tool.

Before retirement, the independently checked migration checkpoint passed 254
native tests, 203 differential groups and 616 Node consumer checks with four
existing/opt-in skips. Those last two figures are historical, not current
coverage, and their removal is not a new passing result.

The Rust-only retirement checkpoint passed **262 native tests** on both stable Rust and
Rust 1.85: 254 library cases, six CLI integration cases and two guidance cases,
with zero failures or ignored tests. Strict all-target Clippy passed on both
compilers. The release `roster` binary also ran help and argument splitting
with an empty executable search path. These checks verify the current native
surface, not the still-missing delivery runtime or legacy coverage as a whole.

Before focused execution review, the composed backend checkpoint passed **283 native all-target
tests**: 261 library cases, 14 owned-backend integration cases, six CLI cases
and two guidance cases, with no ignored tests. This was verified on Windows
GNU with stable release tests and Rust 1.85 debug tests, plus strict all-target
Clippy on both compilers and format checking. The owned Cargo fixtures used
the corresponding installed compiler in each pass. A canonical Windows Cargo
image failure caught during MSRV validation was reproduced on stable, repaired
without changing state-root rules, and paired with a real different-file alias
denial control. These are component/backend results, not full delivery parity.

The focused execution-review checkpoint passed **296 native all-target tests**:
262 library cases, 26 backend integration cases, six CLI cases and two guidance
cases, with zero failures or ignored tests. Stable debug/release and Rust 1.85
debug runs pass on Windows GNU; format and strict all-target Clippy pass on both
compilers. Owned Cargo fixtures use the corresponding installed compiler.
All four reported failures were reproduced before production repairs; earlier
native cases, protected state/planning/transport files and dependency pins were
preserved. Windows worker controls remain distinct from POSIX signal proof.

The archive/restore and explicit-host Docker checkpoint passed **312 native
all-target tests** on stable and Rust 1.85: 263 library cases, 41 backend
integration cases, six CLI cases and two guidance cases, with no ignored
tests. Strict all-target Clippy passes on both compilers; format checks and the
six optimized CLI tests/standalone empty-PATH controls pass. Owned Cargo and
native CLI-protocol fixtures use the selected installed compiler with locked/
offline builds and no new dependencies. Original 296-case sources and reviewed
dependency pins remain unchanged. No live Docker or GitHub publication lookup
was run, and no full rewrite, cross-platform or containment acceptance is
implied.

The focused recovery-review repairs pass **324 native all-target tests** on
stable and Rust 1.85: 263 library, 53 backend, six CLI and two guidance cases,
with no ignored tests. Both reported failures were reproduced natively before
production changes. Twelve added integration cases preserve every earlier
312-case source and exercise the shared complete write-path policy, sealed
artifact absences, actual publication/product-loop barriers, safe owned
rollback, quarantine and failed-full revocation. Strict all-target Clippy
passes on both compilers. No dependencies, policy, workflows, vendor sources
or original test expectations changed. Formatting, six optimized CLI cases
and standalone empty-PATH JSON/invalid-option controls also pass.

The builtin-seat-chat checkpoint passes **358 native all-target tests** on
Windows GNU with stable Rust **1.99.1** and Rust **1.85.0**: 297 library,
53 backend, six CLI and two guidance cases, with no ignored tests or failures.
The 34 added cases comprise 31 wrapper/admission/resource tests and three
planning integration tests. Both compilers pass format checking and strict
all-target Clippy. Owned Cargo fixtures use the corresponding installed
`stable-x86_64-pc-windows-gnu` or `1.85.0-x86_64-pc-windows-gnu` toolchain;
JavaScript consumer fixtures use Node 20.19.5, not a Node Roster runtime.
Six optimized CLI tests and standalone optimized help/JSON/invalid-option
controls also pass with empty `PATH`.

All earlier native test expectations remain unchanged. Four earlier test files
only gain the 17 required `Driver::admission` initializer properties; removing
those added lines reproduces their original byte hashes. The other protected
source changes are the transport observer-evidence handling, module exports and
planning wrapper integration. Dependency pins, root instructions, the
engineering principal, human policy, workflows and vendor sources are
unchanged. These are component results, not full seat-loop or harness acceptance.

The cached-metadata Drop follow-up passes **363 native all-target tests** on
the same stable 1.99.1 and Rust 1.85.0 Windows GNU toolchains: 302 library,
53 backend, six CLI and two guidance cases, with no ignored tests or failures.
Both exact regressions failed before the repair; five barrier-driven tests
were added without changing any of the earlier 358 test sources. Format and
strict all-target Clippy pass on both compilers with matching owned Cargo
fixture toolchains. The six optimized CLI tests and standalone empty-PATH
help/JSON/invalid-option controls also pass. The existing scoped Tokio
instruction captures the Drop-safe metadata learning; its original content,
frontmatter and glob remain unchanged.

One earlier MSRV full run reported `Docker verification deadline expired` in
the unchanged positive Docker protocol test (52 of 53 backend cases passed).
Its unchanged isolated rerun passed in 14.66 seconds, followed by a coherent
363-case MSRV full rerun. No backend source, assertions, deadlines or harness
thread count changed; the initial failure is retained and its cause is not
established.

The native-inner-seat checkpoint passes **409 native all-target tests** on
Windows GNU with stable **1.99.1** and Rust **1.85.0**: **348 library**,
**53 backend**, **six CLI** and **two guidance** cases, with zero failures or
ignored tests. Both compilers pass formatting and strict all-target Clippy.
The 46 added seat cases retain the earlier 363 assertions. The six optimized
CLI cases and standalone optimized help, exact JSON and invalid-option
controls also pass with empty `PATH`. Owned Cargo consumers use the matching
installed outer toolchain; Node consumers use 20.19.5.

Protected source changes for this component are `rust/lib.rs`,
`rust/config.rs`, and execution `mod`, `tools`, `context`, `checklist`,
`continuity`, `finalization`, `self_review`, `archive` and `result`. They add
library exports/profile selection, role-specific context/read access, owned
inner-seat checks, strict checklist inputs/test-alias observations, reusable
current finalization/corrections, actual artifact-write ownership and narrow
owned rotation. No earlier test initializer or assertion was changed.
Transport, builtin chat/admission/metadata-Drop sources, planning/state
sources, dependency pins, principals, instructions/skills, policy, workflows
and vendor sources remain unchanged.

The new whole-flow loopback fixture's initial parallel deadline failure and
isolated rerun are retained in [TESTING](TESTING.md). Final complete stable
and MSRV runs have no failures. These are inner-seat results, not a completed
delivery CLI, full rewrite, cross-platform release or harness acceptance.

The first writer-run focused `df4311a9` repair checkpoint passed **422 native
all-target tests** on the same stable 1.99.1 and Rust 1.85.0 Windows GNU compilers:
**361 library, 53 backend, six CLI and two guidance** cases, with no
failures or ignored tests. Both pass formatting and strict all-target Clippy;
release CLI six and standalone empty-PATH help/JSON/invalid-option controls
also pass. Owned Cargo consumers use each matching compiler.

All three findings first failed actual native execution before production
repairs, including all three malicious author-envelope variants. Thirteen
focused cases preserve the original 409 assertions and distinguish real
Cargo assertion progress, strict tool-free authority rejection and explicit
genuine content/transport advisories. The only existing test-source changes
are the new test-module registration and one additive assistant-role
initializer in the positive composed self-review helper; removing that
property reproduces its original byte hash. The transport/builtin/Drop,
dependency, policy, workflow and vendor pins remain unchanged. See
[the focused failure history and exact commands](TESTING.md).

Its independent follow-up failed: **360 library cases passed, one failed**
and the other targets were not reached. The real Cargo repro stopped at
`RepeatedFailure` but recorded four failures rather than the required three.
The remaining modifier-bearing documentation/web Ask also reproduced an
incorrect actual Pass before its repair. Neither failure is dismissed as a
transient.

The follow-up keeps documentation complements/literals and per-tool negation
nonexecuting while recognizing separately coordinated imperative invocations
with sequencing/adverb modifiers. Cargo failure identity now excludes exact
build-activity and known lock-wait **stderr** records only after an actual
completed native libtest failure. Controlled cache rebuild and concurrent
shared-cache observations established these extra identity deltas; assertions,
failed IDs/counts/profiles, compiler errors/warnings and raw observations remain.
No repair threshold, timeout, concurrency or earlier assertion was relaxed.
All **18 focused cases** pass, including exactly three actual failures and
four model requests in both Cargo repeated-failure repros. The final coherent
checkpoint passes **427 native all-target tests** on stable **1.99.1** and
Rust **1.85.0** Windows GNU: **366 library, 53 backend, six CLI and two
guidance**, with no failed or ignored cases. Format and strict all-target
Clippy pass on both; matching owned fixture compilers, optimized CLI six and
standalone empty-PATH controls pass.

The first follow-up MSRV full run failed one unchanged backend cleanup/deadline
case (52/53 backend passed). Its unchanged isolated run and one complete
coherent recheck passed, without establishing the initial cause or changing
any protected test, deadline or runner setting. That failure and the earlier
web/Cargo failures remain in [the coupled failure history](TESTING.md).
Independent review follows this writing checkpoint; outer orchestration/CLI
and the documented platform/live proof gaps still prevent a full-rewrite claim.

## Builtin seat chat and request admission

Product trace: FEATURE_SPEC 5.2, 5.4, 5.6 and 5.8, subject to sections 3 and 7.
`llm::builtin` provides `default_completion_cap`, `create_builtin_chat`,
`BuiltinChat::complete` and `chat_completion` over the existing native
OpenAI-compatible transport. All native draft, PLAN and outline callers use
this wrapper. The independent plan critic uses it with streaming and length
retries explicitly disabled; a failed critic cannot become planner evidence
or silently start a repair.

The invocation owns one `llm::admission::Admission` and supplies the same
handle to its chats and `planning::lifecycle::Driver::admission`, across tasks
and profiles. Clones share that invocation's registry. Do not create a
registry per chat. A missing profile is not limited; a declared profile
defaults to one slot. Declared concurrency reconfigures that profile in
place. FIFO reservations count as holders even before their futures resume.
Increasing capacity grants queued reservations; shrinking does not revoke
existing holders or admit more until the new capacity permits it. Queue depth
is pending plus holders minus capacity, clamped to zero.

Acquisitions and leases own unique reservations. Explicit release is one-shot,
and dropping a queued, just-granted or active future removes only its own
reservation synchronously. Cancellation preserves the original native reason
and rejects late success. Notifications are registered before checking state;
no admission lock spans an await, and no cleanup task or executor is spawned.
This is request admission, not worker scheduling, a durable queue or a second
task board.

Queued observers receive an immediate waiting event and at most one heartbeat
per 30-second cadence, with current depth, elapsed time and a credential-
redacted host bounded to 256 UTF-16 units. Observer waits are bounded and
cancellable. Queue time is not an HTTP send; the existing transport request
deadline starts after admission, while the invocation's cancellation remains
binding throughout. Transport HTTP/stall retries and builtin length retries
all consume the same unchanged actual-send counter and request ceiling.

The wrapper maps `api_key_env` to transport credentials, defaults optional keys
to true, declares capability-prior/max-token/thinking HTTP metadata and
preserves fleet-locked gateway-label behavior without routing to another
model. Normal default caps are 8192 for low, 16384 for medium/high, 32768 for
xhigh/max and 4096 otherwise, bounded by a quarter of a valid context capacity
with a 4096 floor. Documentation slices instead retain the 8192 completion
floor and disable reasoning, including the local DeepSeek thinking flag.
Explicit streaming and retry permissions remain separate.

Every actual request validates and includes the trusted compiled resource
`principals/engineering.md`: CRLF normalization, JavaScript-compatible trim
and a required 1–8192 UTF-16-unit body. It is appended to the first textual
system message or prepended as a system message, idempotently, without
changing the caller's messages or tools. There is no missing-resource,
current-directory or configured-baseline fallback. JSON and Text remain
lossless for lone surrogates in messages, tools, partial output and responses.

A length finish permits one retry per request and at most three per chat
lifetime. Visible partial code/tool output is not replayed. A nonempty
documentation-only text partial may continue, but tool calls never do.
Reasoning-only output may double the persistent cap up to 32768 without
shrinking an already higher cap. Corrections forbid repeating executed edits.
Unsupported finishes and explicitly disabled retries retain their original
partial failure evidence.

`last_attempts` counts wrapper attempts, not HTTP sends. `last_usage` aggregates
reported usage, keeping absent counts unknown and real zeros zero; overflow
fails instead of inventing a total. `last_response` remains the actual latest
response's model/usage, not the aggregate or a configured/declared substitute.
Parsed response evidence survives non-cancellation observer failures.
Cancellation/deadline failures cannot install a newly revoked response, and
critic evidence remains separately scoped. A locked route intentionally uses
the requested model label, with usage still supplied by the actual response.

Before another transport attempt is awaited, its aggregate usage becomes
unknown rather than retaining the first partial response's counts as a total.
Response evidence remains pending while its terminal observer is awaited;
dropping that future, including cancellation followed by immediate Drop without
repolling, preserves the prior snapshot. Settled non-cancellation errors still
publish the actual parsed response, including observer errors and aggregate
overflow. This needs no detached cleanup and holds no synchronous lock or
interior-mutability guard across an await.

The wrapper remains transport/request admission, not another worker runtime.
The native inner seats below compose it; outer delivery CLI, provider
discovery and publication adapters remain separate migration work. The later
native-vault section records selected file-backed credential integration.

## Native inner coder and reviewer seats

Product trace: FEATURE_SPEC **5.4 execution**, **5.5 review and gates**,
5.1/5.2 authority and staffing, 5.6 measured learning and 5.8 ownership.
Sections 3, 7 and 8 still apply. This is a library component, not full delivery
or harness acceptance.

`seats::Driver` borrows the invocation's transport, observer, credentials,
cancellation and the same `llm::admission::Admission` used by planning. It
creates no runtime, chat worker, scheduler, second board or parallel writer.
The caller supplies one current-thread invocation and an already opened
`execution::tools::Boundary` for the strict unchanged TASK/recipe/assignment.
Coder and reviewer use that same writer lease sequentially:

```rust
let coder = driver.run_coder(&mut boundary, repo, &config, coder_options).await?;
let review = driver.run_reviewer(&boundary, &coder, repo, &config, review_options).await?;
review.require_passing(&boundary, &coder, &projection).await?;
```

The reviewer receives only an immutable boundary borrow. Its `ReadOnly`
facade accepts read/search/diff requests; it cannot expose the mutable coder,
tests, shell, merge or publishing tools, or acquire a second lock behind the
first. The caller retains ownership until its eligible work and cleanup end;
unknown cleanup cannot be turned into a successful close. Outer planning/
worktree preparation and lease transitions are still the caller's job.

### Coder

The configured coder freezes actual assignment, source baseline, configuration,
bounded context, principal and complete selected skills before product work.
Native paths also require complete Rust/Cargo guidance; asynchronous seat,
execution and transport modules select Tokio guidance. Changed instructions
revoke the context. An anticipated native expansion refreshes complete guidance,
revokes the pending batch and requires new calls; it never grants a path.
Only the host's current owned CONTEXT is rotated, through the existing private
Git-common archive, without moving another human artifact.

Host/configuration/recipe permissions, builtin principal, required verification
images, selected web/native-CLI prerequisites and Design grounding are checked
before implementation. Selected unavailable capabilities stop explicitly.
Check triage observes real approved matching checks and source exports; an
export alone is not proof of behavioral prose. Selected research is a bounded
read-only inventory, not a claim that implementation or acceptance ran.

The streamed loop offers only intersected structured functions, decodes bounded
arguments/IDs with the existing correction budgets and executes each accepted
current-batch call once. Reused gateway IDs are normalized consistently in
assistant and tool messages. Prose cannot execute. Read-before-write/edit/
delete, protected/private paths, required earlier-wave reads, scope limits and
effective written/deleted Node/Cargo coverage remain enforced. Secret-bearing
content is refused before writing and receives at most one correction; that
does not soften a path or capability denial.

Named-file no-ops, missing summaries, stale/open checklists and required
one-line README Status content receive bounded corrections, not deterministic
code fallback. Observed checklist closures bind actual current read/write/test
references; models cannot supply observation IDs. Source changes revoke old
closures. Scoped tests and the restricted `run_command` test alias share the
same observation and repair accounting.

Final matching/full checks run through the existing Windows owned-process
backend, including ordinary coder Node/Cargo checks. They require absolute
approved images, physical executable identity and complete owned shutdown.
Selected strict Unix checks remain unavailable rather than run a direct-child
fallback. Dropping an active check revokes success and retains the writer for
recovery, even when the owned process handles subsequently signal termination.
Cargo output must use an owned target outside the product worktree: source
sealing does not silently exempt arbitrary ignored build files.

Actual test failures retain duration-normalized failure fingerprints and
bounded progress/repair budgets, up to the existing cap. Measured context use,
repeated failures, policy denial, scope exhaustion, turn/context budgets and
unavailable capabilities return explicit unverified handoff data. The loop
does not implement outer rerouting, rescoping, perspective escalation or
review-repair orchestration.

Default finalization composes current private test evidence, complete failed-
full obligations, separate tool-free self-review, substance, shadow,
counterfactual, trusted hooks, read-only acceptance and selected independent
verification. Applicable corrections require new current source and checks.
A green scoped call cannot erase an actual failed full suite; only a current
full pass or the existing complete qualified outside-failure exception can
discharge it. Configuration/transport/observer/finish/timeout failures and
cancellation cannot become a deterministic passing result.

No endpoint selects only a truthful stub RESULT and context. It makes no
product edits, model calls or test executions, emits no response-backed AI-Run
and cannot feed a passing review.

### Reviewer and artifacts

Review starts with fresh bounded chat context, not the coder transcript. It
uses the reviewer principal, authoritative Original Ask, exact checks,
unchanged HEAD/import sources, earlier waves, current RESULT, full safe actual
diff and current host observations. A separate Original Ask audit withholds
the planner checklist/coder summary so they cannot weaken missing outcomes.
Negative audit grounds bind a verbatim authoritative obligation and exact
delivered-source excerpt; null is permitted only for an actually absent file.

There are no function tools. At most three JSON request rounds, each with
1–4 bounded read/search/diff requests, precede a strict bounded verdict.
Every check and previous finding needs a unique covered ID; unresolved
findings, unmet checks, concrete changed-file defects, failed host observations
or missing Ask outcomes prevent a pass. Unsupported functions, denied requests,
bad schema/grounding, cancellation, observer/transport failure or changed
source produce an unverified REVIEW, never a stub pass.

`CoderResult` and `ReviewResult` have private constructors/state.
`ReviewResult::require_passing` rechecks the actual owned RESULT/REVIEW
identities and bytes, writer, TASK/recipe, source, HEAD, context and selected
private host receipts. A model verdict or serialized receipt cannot construct
this authority. Successful RESULT ownership comes from the actual exclusive
write, not a later filename-based adoption. Existing human files are refused/
preserved. Identity-checked narrow rotation archives only a current host-owned
non-authority artifact; ordinary full-archive/restore absence seals are unchanged.

The focused `df4311a9` review repairs bind prerequisite negation to the
particular tool invocation rather than an unrelated prohibition elsewhere on
the line. The selector recognizes bounded explicit invocation/list forms;
literal examples, documentation descriptions and fenced samples do not grant
internet permission. A required unavailable web capability still stops before
model admission or product writes even when internet configuration is disabled
or otherwise enabled.

Repair fingerprints normalize actual Cargo `Finished … in …`, libtest result
elapsed fields and Node timing records. Failed test IDs, result counts,
profiles and assertion/output differences remain inputs to progress. Actual
Rust failure controls exercise repeated-stop and single-file budget bounds;
changed public-operation assertions still earn bounded extensions and finish
with real Cargo green. No repair budget or escalation ceiling is disabled.

Author and independent reviewer responses use one tool-free assistant-envelope
validator. A user/tool/system/missing role, function/tool request or invalid
finish cannot yield a clean author report. Shared author acceptance preserves
the actual invalid response model/usage and original authority error in a
distinct `rejected` report; even an unchanged current source cannot make that
report passing evidence. The advisory adapter cannot erase that rejection.
Explicit genuine non-cancellation transport or content-parse advisories retain
their existing `unavailable` contract, not a fabricated clean verdict.
Cancellation, stale-source and configured-loop failure behavior remain
separate and unverified.

Run metadata comes from actual settled responses; real zeros remain zero and
missing usage remains unknown. Coder, author self-review and Original Ask
auditor evidence remain separately scoped. Neither a model label nor principal
text grants App publication authority. First failure/cancellation reasons and
observed failed-full exits remain visible; stop notifications are bounded.

`Recording::default()` explicitly chooses session-only operation, without a
durable-history claim or repository fallback. A supplied repository-bound
machine store records redacted scalar outcome/model/usage, stage references
and failure names/codes, never source bodies or transcripts. Only an explicit
host `promote_summary` selects a compact rule-approved outcome extract through
the existing curated-memory API. Ordinary raw history never enters context;
memory loading remains repository- and seat-bound. Recording failure archives
only the actual owned result/review and leaves an unverified diagnostic, not a
passing continuation or silent persistence fallback.

## Execution and ownership

The strict boundary freezes task authority, permissions, actual executable
approvals, physical worktree identity and writer instance. Model output,
skills, principal names and transcript text do not grant tools or publication.
Only a current decoded structured call batch can execute.

Verification uses the effective planned/concrete/expanded/repair/written and
deleted scope. Mixed Rust/JavaScript scope needs actual approved Cargo and
Node evidence. Skips cannot mint test passes; stale sources revoke evidence.
Aggregate checkpoint scope is bounded before mutations. Full evidence and
tracked/untracked diff limits fail explicitly instead of truncating sources.
Product deletion remains a product change for counterfactual applicability.
Historical source lookup uses the captured HEAD, not index membership.

Locks preserve malformed or ambiguous owners. Age is not death proof, and a
changed file identity cannot authorize another owner's release. Windows
owner-read access/sharing failures may wait only within the configured budget;
expiry returns the actual error, cancellation preserves the marker, and a
subsequent acquisition still requires exclusive creation or death-proof
takeover. The real sharing-handle regressions cover release, bounded wait and
cancellation; they do not prove every access-error or mixed-version race.

Private state and migration obey [STATE](STATE.md). No migration command has
moved real user data during development. Unknown usage stays unknown.
Response-backed measured run objects override later configuration declarations.

### Approved execution backends

FEATURE_SPEC 5.4/5.5 is implemented here as library boundaries, not a second
seat loop. A host opens the existing task boundary, begins a frozen
`execution::finalization::Attempt`, then calls `approve_backends` exactly once
before context, responses or tools. `execution::backend::Approval` names the
approved Node consumer executable, human-hook executable, native CLI image,
local submodule sources and optional operation timeout. Counterfactual and
failure-classification execution each require their own explicit opt-in.
No model call, transcript, script string or general shell permission grants
that authority.

Canonical Windows executable paths reuse native verbatim-path normalization
only after binding the selected and checked file identities. Both path
identities and the image metadata are revalidated after execution; a different
physical image behind a normalized alias is denied, not substituted.

The normal `run_test`/`verify_full` path still requires actual approved engine
coverage. Backend reports are privately constructed from observed subprocesses,
bound to the current writer, authority, physical source snapshot and Git
state. Call `require_current` before reusing a report. Finalization composes
applicable counterfactual, post-coder hook and end-to-end gates and renders
their real exits and bounded/redacted output. Stub, cancelled, failed,
unavailable, skipped and successful observations remain distinct.

An actual failed or unavailable full verification remains an obligation in the
writer boundary, independently of `last_tests`, context refresh, source edits
or a replacement `Attempt`. A scoped pass cannot erase it. Resolution requires
either a current actual full pass with every required engine covered, or a
complete current outside-failure exception tied to the exact private full-run
observation followed by real scoped passing evidence. A new full request
revokes the preceding completion until the new request finishes. Stale full
passes and stale classifications do not survive source/Git changes. RESULT
retains original/latest failed full exits and distinguishes actual full
recovery, a qualified outside exception and an unresolved obligation.
These in-process ownership rules also survive native restore revocation;
serialized JSON or a scoped green checkpoint cannot mint full-run authority.

- **Owned baseline:** a bounded private container in the repository's Git
  common directory contains a local shared no-checkout clone at the captured
  revision. Native blob materialization avoids checkout filters. Changed test
  files, not candidate production or manifests, are overlaid on that base.
  Local gitlinks require exact path/source approval and matching source HEAD;
  nested local approvals are explicit. No submodule URL is fetched or updated.
  The retired Node Roster package is refused before materialization; this is
  not a frozen legacy checkout or application fallback.
- **Named red/green:** explicitly approved Node 20 consumer tests and direct
  Rust `tests/*.rs` integration targets run on base and candidate. A red
  observation needs a real named failure and positive ordinary exit; skips,
  missing names, compiler/import/setup failures, signals and resource limits
  are not red tests. Cargo runs locked/offline with separate owned targets.
  Rust mixed production/inline-test counterfactuals are unavailable rather than
  copying changed production onto the base or silently omitting verification.
- **Failure classification:** only current private failed-test evidence can
  start actual isolated reruns and base comparisons. Reports distinguish
  transient, preexisting, regression and unclassified failures. A proven
  outside-scope Node failure still needs a fresh real scoped test before
  finalization can pass; its original full-suite exit remains visible.
  Every observed failure must map uniquely and completely, including other
  failures in the same file. Unknown/dynamic names, duplicate declarations,
  unsupported failure metadata, asynchronous setup errors, cancellation and
  incomplete output are not whole-file preexisting proof. Fully mapped
  ordinary/nested failures still get actual comparisons. A separate
  zero-selected-test Node startup control exposes uncontrolled setup/worker
  termination masked by another named failure; it is only a negative control,
  never passing test or engine-coverage evidence. Cargo failures never waive
  required Cargo checks, including mixed Cargo/Node tasks.
- **Lifecycle hooks:** manifest and scripts must be stable single-link regular
  files matching captured committed HEAD. Human Node hooks run with a minimal
  credential-free environment, bounded time/output, source/Git mutation
  detection and owned descendant cleanup. An absent manifest is genuinely
  inapplicable. Before TASK exists, open `backend::RootLease` and call
  `hooks::run_pre_plan`; the existing planner's
  `run_planner_owned_after_pre_plan` checks the current receipt and borrows that
  exact lease token. The original planner entry retains its no-hooks guard.
- **Read-only CLI acceptance:** an explicitly approved native binary runs the
  selected authoritative checks with isolated home/config/state directories
  and an actual unknown-option negative control. Help, recipe validation,
  catalog parsing and argument splitting use native code. Selected missing
  commands, forbidden authority, malformed JSON, crashes and exceeded bounds
  cannot become “no commands” success. Historical command spelling may express
  native intent, but never invokes a removed Node Roster implementation.
  Windows `roster.exe` spelling, casing and quoted launchers retain their CLI
  obligation. Every recipe operand, including option-shaped or additional
  operands, is checked against exact readable TASK/harness scope and protected
  path/containment rules before the approved native image can execute.

The strict owned runner currently has an actual Windows GNU backend. It starts
the target suspended, assigns non-breakaway Jobs before resuming, bounds the
combined stdout/stderr budget, and does not accept direct-root exit as
descendant completion. Kernel Job membership is checked together with pinned,
signalled process handles; each handle's membership is rechecked after opening
to reject PID reuse. Cancellation, timeout and overflow terminate and wait for
the owned Job. Unverified shutdown retains scratch and the writer marker;
cleanup errors and late cancellation revoke success.

### Artifact archive and transactional restore

The host-only `execution::archive` API retains the legacy Git-common layout:
`roster-artifacts/<issue-or-local-task>/<milliseconds>-<12-hex>/`. Other opaque
task names use the existing `task-<16-hex-hash>` directory convention. The eight
known planning/run artifact names and bytes remain compatible with legacy
readers. Active task boundaries always preserve TASK, recipe, plan and estimate
authority. A pre-plan root lease can archive earlier handoffs before a new TASK
exists. Tracked, linked, changed, oversized or secret-bearing artifacts fail
closed. Copies are private/exclusive, source removal is identity-checked, and
rollback never overwrites a newly created user artifact. Matching-task archive
reads are bounded untrusted context data, not automatically promoted memory or
new tool authority.

Preparation seals both present artifacts and the absence of every
non-preserved artifact name. Confirming an absent RESULT, REVIEW, CONTEXT or
RESEARCH never approves a later human file. Absence is rechecked before
archive/staging, after asynchronous publication and product-loop barriers and
through commit, including when there was nothing to archive. Late human bytes
remain untouched; owned writes roll back. A new human replacement at an
already archived name preserves both versions and quarantines the writer if
the old artifact cannot safely be restored. Missing preserved authority
artifacts are not implicitly added to the archive set.

`checkpoints::prepare_restore`/`restore::prepare` reads and validates a verified
candidate without mutating products. Its sealed plan binds the current writer,
physical worktree/common-directory identities, strict issue branch, HEAD,
checkpoint ref/tree/metadata identity, unchanged task/recipe/assignment, exact
scope, main Git index and complete current product/audit snapshots. Products
are regular single-link unprotected files; linked trees, secret material and
implicit file/directory transitions are refused. The 64-scope, 2048-file,
16-MiB-file and 128-MiB combined source/target bounds are checked before restore.
Every target uses the same complete normalized, contained private/protected
write-path policy as ordinary coder writes before a restore plan is accepted.
Even an explicitly approved unpublished checkpoint and broad task scope
cannot grant writes to private configuration, another worktree, dependency
directories or protected aliases.

Only `Plan::confirm(..., Publication::Unpublished, true)` creates a one-use
approval. Execution additionally requires a host `PublicationCheck`, rechecked
before preparation side effects, immediately before product mutation and before
commit, with a bounded 30-second deadline per lookup. Expensive local staging
does not reuse a cached publication answer or silently expire into weaker
authority. Source/ref/index checks run again after the lookup. The native
library does not query GitHub, infer publication from local refs or turn an
unknown/failed lookup into “unpublished.” Unknown, published, unconfirmed,
stale-source and stale-writer approvals refuse; publication changing during the
transaction rolls back owned writes. This API is not exposed in model tools.

Execution saves later scoped tracked/untracked edits as a new before-write
checkpoint, archives stale run artifacts **before** product restoration and
stages a private transaction under the worktree checkpoint directory.
Exclusive hard-link installation avoids overwriting a racing user path and
retains original file identities for rollback. It leaves the main index and
all unapproved paths alone. Target and backup refs/metadata, parent identities
and product bytes are revalidated through commit. Owned source backups and a
bounded journal remain available for inspection; COMMITTED is a restored
candidate, never a test-pass receipt. A ROLLED-BACK marker records owned product
rollback. Unknown rollback/cleanup retains the writer and evidence rather than
deleting foreign files or claiming success.

RECONCILED is written only after both source and artifact rollback finish.
On a later boundary open, private journals rebind the physical worktree and
transaction identities and restore only negative obligations: a fresh context/
check requirement, revocation generation and any earlier failed-full exits.
They never restore a passing test receipt. A missing, malformed or
unreconciled transaction blocks normal use and retains the new writer marker
for explicit operator reconciliation; a stale COMMITTED marker cannot hide an
unreconciled rollback. Automatic repair of an interrupted journal and its UI
remain operator/outer-orchestration work, not an inferred success path.

Restore revokes the old authority generation/writer instance, pending
attempt/workspace use, prior test and self-review receipts and required-read
caches. New context and actual fresh matching checks are required. Previously
failed full-suite obligations remain; restoring an earlier full-pass candidate
does not supply a current full receipt. Cancellation/foreign task changes still
permit only physical-owner-checked rollback of owned writes, never replacement
of a human's changed TASK. These are native library interfaces; the resume CLI,
operator recovery UI and outer worktree orchestration are not implemented.

### Explicit local Docker independent verification

`backend::Approval.independent` distinguishes genuine absence, selected Docker
without approval and a frozen explicitly enabled `docker::Approval`.
Selection without executable/image/backend/daemon capability is unavailable,
not a pass. A plan names an absolute host CLI, local Windows pipe or Unix
socket, immutable locally present `sha256:` image ID, fixed non-shell native
entry point or external consumer Linux Node 20 runtime, exact input/dependency
paths, host-owned observations and optional verifier sources. Native image
programs do not invoke a Node Roster implementation. Nothing is loaded from
model instructions and there is no pull, build, login or registry fallback.

The backend captures the existing version-one task/RESULT/product subject,
plus strong current-source and dependency/plan bindings. Inputs are copied
into bounded readonly snapshots in owned Git-common scratch. Product plus
dependency inputs total at most 64 paths/4 MiB, with the existing subject's
64-KiB per-file bound. Host verifier sources are limited to 16 simple names,
64 KiB each/256 KiB total. Each of 1–16 actual observations must cover an
authoritative acceptance check and fit the receipt's exact output/exit limits.
Zero-selected Node startup checks elsewhere are not independent evidence.

The host CLI uses isolated home/config/temp directories, empty auth
configuration and an explicit local endpoint, not ambient Docker contexts or
credentials. Image inspection must confirm the exact Linux image and safe
environment before container creation. Each container has a unique name,
CID file and full ID with owner, subject and plan labels. The CLI executable's
identity/size/timestamp is pinned across operations.

Creation uses pull-never, network-none, readonly root and bind mounts, all
capabilities dropped, no-new-privileges, UID/GID 65534, 256-MiB memory/swap,
one CPU, 32 PIDs, nofile 256, private cgroup and non-host namespaces, IPC none,
restart-no, logging-none and a bounded noexec/nosuid tmpfs. Inspection must
confirm the exact command, environment, mounts, isolation/resource settings
and ownership before start and after exit. Infrastructure/signal/OOM,
timeout, overflow, malformed metadata and stale subjects cannot be passing
observations.

The deadline is bounded and cancellation is revocable. Cleanup has a separate
deadline, ignores cancellation only for fixed physical-owner-checked cleanup
commands, removes by a confirmed owned **full container ID**, and independently
confirms absence. An unconfirmed create/CID, foreign labels, ambiguous absence,
changed CLI or failed removal retains the writer and snapshots. Names alone
never authorize deletion.

`Attempt::finish` composes the selected backend after default native gates.
For a new RESULT, it freezes candidate bytes before independent verification;
receipt metadata remains separate to avoid recursive subject hashing.
Passing `Finalized::write` creates those bytes exclusively and revalidates the
subject. Public current-receipt use then requires the unchanged actual RESULT.
Existing different user results are preserved/refused, not overwritten.
Docker failure/unavailability changes the final verdict and cannot leave a
verified continuation capsule.

Migration tests execute only dependency-free owned Rust Docker CLI protocol
fixtures and real owned filesystem/process controls. **No live Docker daemon
or container was used.** These tests do not prove Linux/container containment,
Docker Desktop path translation, daemon trust or host ACLs. Production requires
explicit host enablement and a separately approved local image/daemon.
Non-Unicode/separator-ambiguous mount paths and unverified platform/backend
capabilities fail explicitly rather than weaken mount identity checks.

Real Windows controls cover a root exiting while a detached descendant lives,
natural descendant completion, descendant termination on timeout/cancellation/
overflow, aggregate output and spawn failure. They are not Unix proof or an
entire filesystem/network sandbox. On other platforms selected strict owned
execution is explicitly unavailable before spawning; no detached direct-child
fallback is used.

## Native local delivery checkpoint

Product trace: FEATURE_SPEC **5.4 execution**, **5.5 review and gates**,
**5.7 human interface** and **5.8 operation**, with 5.1 authority, 5.3 bounded
intake and 5.6 truthful provenance. Sections 3, 7 and 8 remain binding. This
checkpoint is not the complete rewrite or GitHub harness acceptance.

`config::load` selects canonical target-repository private config before
installation private config. `paths::repository_paths` follows checked Git
worktree/common-directory pointers and registration backlinks; linked
worktrees share the canonical private source. Missing nonrequired config uses
the embedded example through the real validator. Invalid, unreadable,
nonprivate, linked or multi-link settings do not become defaults. The loaded
source/precedence and selected critic fleet bytes/identities are sealed and
rechecked at observation/stage boundaries.

Environment overrides, installation resources, exact timeout ranges and
explicit image/source approvals are documented in [INSTALL](INSTALL.md).
No invocation changes global cwd/environment or logs configured endpoints,
model labels or secret values. Native credentials resolve only explicit
selected environment-key names at this initial checkpoint; its vault provider
was not yet implemented. The native-vault section below records the later
file-backed integration.
Retention loading reuses the existing validated policy/defaults and performs
no cleanup.

`start` resolves current HEAD, an unambiguous tracked ref/complete commit SHA,
or trunk through origin HEAD/main/master. Fetch is a separately approved host
operation; offline/no-origin starts state freshness is unknown. The fixture
fetch policy accepts only an exact approved local origin/HEAD. Production
fetch accepts explicitly enabled credential-free HTTP(S), not arbitrary
helpers, credentials or `.gitmodules` URLs. Missing objects/ref ambiguity,
source/config/ref drift, malformed inventory and namespace collisions fail.
There is no reset, force-push, human worktree adoption or automatic removal.

Local preparations are real registered Git worktrees at the selected SHA,
with `agents/local-<host-id>` branches, bounded source trees and private
Git-common claims. Issue/legacy branch inventory is read-only; recognizing
an issue-shaped name grants no GitHub or publishing authority. Dirty caller
source and unrelated human worktrees are preserved. Existing-TASK mode stays
in its current worktree and refuses dirty products/foreign result artifacts.
Selected retired Node Roster bases are refused before materialization.

Declared submodules require explicit local-source approvals, exact source
HEAD/gitlink equality, bounded nesting and safe local Git configuration.
Git-only shared-object checkout does not edit/copy the source pack or rewrite
declared URLs. Missing/stale/extra approvals fail before claiming a new tree.
Lazy fetch, replace objects, hooks, filters/includes and credential/custom
execution paths are not a source of implicit authority. The installation's
contracts dependency must be complete, clean and at its reviewed gitlink;
its physical publisher marker is sealed without invoking the SDK.

`delivery::Driver::run_local` uses the invocation's single current-thread
runtime, transport, observer, typed credentials and shared `Admission`.
Ask-file mode runs actual planning/critique in the prepared worktree.
Existing-TASK mode validates the unchanged Original Ask/scope/checks, accepts
an unchanged canonical recipe or exclusively creates a missing one, and
writes the estimate. Neither mode silently replaces a human artifact.

The root writer is acquired **before** pre-plan hooks or generated TASK
authority. The session-only planner entry borrows the exact pre-plan receipt
token and does not read/append legacy installation memory. The same lease is
atomically moved into the strict execution boundary, with no unlock gap or
second lock acquisition. Coder and fresh read-only reviewer use that boundary
sequentially and the existing private RESULT/REVIEW/source/test seals.
Required review can be skipped only by both an explicit invocation flag and
private `review.required: false` preference. This qualified local skip grants
no App/policy capability or publication authority. There is no automatic publication.

The coordinator calls actual `run_coder`, `run_reviewer` and
`require_passing`, including matching Node/Cargo coverage, complete failed-full
obligations, counterfactuals, hooks and selected read-only acceptance. No
serialized/model verdict replaces those receipts. Failed review, unmet Ask
outcomes, stale source/configuration, cancellation, missing engines or unknown
cleanup cannot become verified delivery. The fixed-profile outer continuations
below now consume those private seat APIs; automatic fleet routing, source
restoration, resume and GitHub publication remain unavailable.

The caller owns `delivery::Ownership` outside the running future, including
bootstrap Git uncertainty and in-progress lease transfer. After a
model future is cancelled/dropped, it can await known cleanup and close the
same retained writer; admission/body ownership is independently released.
Dropping an active owned hook/check quarantines the writer even after its
Windows process handle signals termination. Git/preparation uncertainty
retains its claim for explicit recovery. Age never proves death.
Dependency revalidation precedes the final private coder/review gate; no
awaited dependency work follows the declared completion event. A pending
metadata/handoff future cannot drop its uncertainty and then release a writer.

The actual CLI uses `with_task_deadline` and Tokio Ctrl+C notification, then
**awaits** owned work and cleanup. Heap-pinned invocation/seat futures bound
Windows debug stack frames without a new executor or changed global stack
setting. Exit 124 is task timeout, 130 is cancellation, and ordinary invalid/
unavailable/unverified operations exit 1. Stub/planning-only and explicitly
review-skipped operations are qualified outputs, never passing-review or
publication authority. Artifact/worktree/log paths are reported.

Session logs are exclusive, identity-checked, bounded per-worktree JSON lines:
phase/status, sequence and actual available usage only. They contain no raw
request, transcript, scoped source or configured route labels. Actual response
model/zero/unknown evidence remains in native seat records and owned artifacts.
Every output explicitly says **session-only; durable history was not recorded**.
No machine history, transcript promotion, routing ledger or second board is
implied. Vault credentials now have their separate file-backed provider below;
they are not a run-history store.

`init` exclusively creates the config example and human-policy note, keeping
existing regular files; it never writes actual policy, workflows, credentials,
private config or ignore rules. `doctor` checks offline prerequisites/config/
reviewed contracts and selected image presence without model/GitHub requests,
App-value printing or an image version assertion based on filenames.

The old help's blanket delivery-unavailable sentence is intentionally replaced
by implemented local modes and explicit outer/GitHub exclusions. The original
unsupported `run --issue` negative remains. Legacy `--ask-file` was a forced
stub demo copy; this checkpoint intentionally makes it a real isolated local
task with a configured native positive path. There is no demo/Node fallback.
Local output deliberately adds explicit verified/unverified/stub/planning-only/
review-skipped modes, recording/publication qualifications and current artifact
locations. Those changes are not presented as byte parity with the old demo.

### Ctrl+C dependency rationale

The standard library has no safe cancellable console-notification API. The
existing pinned MIT Tokio **1.53.3** gains only its `signal` feature for
`tokio::signal::ctrl_c`. Signal-hook-registry, errno, libc and the selected
Windows bindings were already in Cargo.lock through the existing Tokio
process graph: no crate version, checksum, new executor or application FFI/
unsafe code is added. Current-thread invocation and owned cleanup remain
mandatory. Actual console-event injection on other platforms remains a
separate validation gate, not a claim from deadline/unit controls.

### Checked initial checkpoint

The final serial frozen-source checks passed **463 native all-target tests**
on both Windows GNU stable **1.99.1** and minimum **1.85.0**, with matching
installed Cargo consumer toolchains, Node **20.19.5**, format and strict
locked/offline all-target Clippy. Each matrix retains the previous 427 cases
and adds 36 invocation/local-delivery cases. Optimized CLI/local targets pass
**42 cases**; standalone help/argument parsing also works with empty `PATH`.
There are zero failed or ignored cases in these final gates.

See [the exact matrix, retained development failures and remaining platform
gaps](TESTING.md#verified-local-delivery-matrix). This is a verified local
component checkpoint, not GitHub orchestration, human acceptance, an App
publication, Windows ACL/Unix containment proof, a release or a speedup claim.

### Independent review 49440ec5 follow-up

The parent independently reproduced the initial **463-case stable green**
before the writing handoff. That green did not cover four subsequently
source-confirmed defects. Native pre-fix controls then observed all four:
real clone/checkout faults lost the runtime writer, a selected ordinary
`CRITIC_AUTH` value entered managed artifacts, a valid keyless critic was
rejected and an actual HTTP timeout exposed its authority. The initial
five-control run had four failures and one already-passing callback control;
it is retained as failure history, not counted as acceptance.

`start::prepare_local` now receives caller-owned `start::Preparation`.
The returned `Prepared` is metadata, not an owner. Admission state and the
actual new-tree `RootLease` live in that external holder before approved
submodule clone/checkout or verification awaits. A materialization Drop guard
quarantines the actual lease on interruption or Git uncertainty before any
release is possible. Failure no longer closes an unrelated clean lease and
checks Git uncertainty afterward. Physical process termination alone does
not clear uncertainty. The delivery coordinator holds this same preparation
through its existing strict boundary transition.

This is a necessary typed library integration: callers pass the preparation
holder and close it explicitly; the former returned `Prepared.lease` field
is removed. Existing tests retain their original assertions and replace only
that ownership initialization/close path. It adds no task database, writer,
runtime, permission or automatic recovery.

Critic credentials come from the **effective** `Config::with_profile`
configuration, including its existing key-name fallback and optional-auth
inheritance, not a required raw profile field. Only resulting selected names
are read. All selected credential values populate a dedicated shared
redaction source regardless of their variable spelling; no name is renamed
to trigger a heuristic. That source protects assignment/planning/product
artifact checks and private diagnostics while retaining the existing
general-purpose redaction behavior.

Endpoint diagnostic forms use the same native endpoint construction and URL
normalization as transport: canonical hostname, authority, explicit/default
ports and IPv6/case forms for primary and effective critic routes. These
forms apply to errors, not arbitrary product source or legitimate measured
model provenance. Error names/codes, absent codes, first cancellation reasons
and actual response/zero/unknown evidence are retained. Planning/seat error
boundaries sanitize diagnostic messages before failed artifacts can persist
them; the low-level transport contract is not replaced with a fake provider.

The additional controls use real approved local Git actions behind an owned
dependency-free native phase fixture, actual kernel process/descendant
handles, owned loopback HTTP/SSE and native request timers/callbacks. They
exercise success, known failure, awaited cancellation, immediate Drop,
keyless/inherited/custom/missing/invalid auth and both endpoint roles.
No submodule URL, external model, GitHub, hardware or Docker operation is
authorized. Original reviewer confirmation remains required before a later
outer-orchestration scope.

The repaired frozen source passes **474 native all-target cases** on stable
**1.99.1** and minimum **1.85.0**, with format/strict all-target Clippy and
matching installed consumer toolchains. Release CLI/local coverage is **53**
cases, including the original 42; empty-PATH standalone smoke also passes.
All prior 463 assertions/cases remain. An unchanged two-second legacy PID
readiness control failed in the first MSRV full run; its unchanged isolated
pass and subsequent complete coherent 474-case pass are retained separately.
See [the repaired matrix and full failure history](TESTING.md#verified-repaired-matrix).
No new dependencies, permissions, policy/workflow/vendor changes or review
acceptance are inferred from these gates.

### Non-HTTP seat observer persistence follow-up

The original reviewer subsequently confirmed the lease, selected-credential
and effective-critic repairs, but found a remaining diagnostic path not covered
by HTTP callbacks. A successful completion followed by a failed `seat-tool`
observer could persist selected endpoint metadata in RESULT's first failure.
A failed `seat-stopped` notification also retained unsanitized metadata.
The parent's independent **474-case stable green** preceded this follow-up.
Two actual delivery regressions failed before its behavioral fix.

Both shared seat-driver methods now apply the existing diagnostic error
sanitizer before returning to coder/reviewer terminal construction. This
includes observer, timeout, authority and cancellation error branches, without
changing their ordering, names, codes or cause chains. Primary/effective-critic
endpoint forms, diagnostic model labels and all explicitly selected credential
values remain private; actual response-backed model/usage fields are separate
and are not blanket-redacted. No CLI-only catch or artifact-wide model masking
substitutes for this source boundary.

Six additional native delivery cases cover first-tool failure, original
turn-budget failure plus stop failure, actual reviewer fail/invalid-response
notification paths, healthy passing delivery, non-green model advice and
cancellation competing with a ready successful callback. The scripted transport
never resolves or contacts configured non-loopback metadata labels. Product
trace remains FEATURE_SPEC **5.4 execution / 5.5 verification**, with no new
runtime, permissions or outer orchestration. See
[the follow-up evidence](TESTING.md#non-http-observer-persistence-regression).

The serial frozen-source stable **1.99.1** and minimum **1.85.0** campaigns
each pass **480 native all-target cases**, formatting and strict locked/offline
all-target Clippy. All prior 474 cases remain; no old assertions or typed
initializers change in this follow-up. Optimized CLI/local coverage passes
**59 cases**, retaining the original 53, and standalone help/exact argument
JSON pass with empty PATH. These are component verification gates, not the
original reviewer's independent confirmation or human acceptance.

### Independent Docker deadline failure: retained diagnostic checkpoint

The parent's later independent 480-case run failed the existing owned Docker
deadline/overflow/cancellation cleanup control: 366 library and six CLI cases
passed, then 52 backend cases passed and one failed; local-delivery and
guidance targets were not reached. The error retained the writer and scratch
after unverified container cleanup and a verification deadline. The failed
mode, submission/CID state and cleanup phase were not recorded. Earlier
producer passes do not resolve or supersede that failure.

Focused diagnosis first ran the exact unchanged control (pass), then seven
Docker-only controls and the 53-case backend campaign with bounded native
fixture phase diagnostics (both passed). These are observations, not a root
repair or a new 480-case acceptance claim. One observed timeout had a created
CID and owned cleanup but no logged start/ready barrier; whether a start
process was submitted cannot be inferred from that missing fixture event.
No five-second verification budget, independent ten-second cleanup budget,
thread count, assertion or production ownership rule changed.

The diagnostic-only harness records bounded static protocol phases, native
PIDs/timestamps, full-ID command selectors, retained CID/intent state and
error classes/digests under `.docker-480-evidence`. Returned failures retain
the owned fixture root instead of letting its ordinary fixture destructor
erase the evidence. Raw credentials, endpoint metadata and error bodies are
not diagnostic payloads. That handback made no production fix: the exact
parent's failed protocol/process phase remained unestablished at that point.
The later retained reproduction and repair below do not erase this history.

### Owned create-submission repair

The parent's next original default 480-case run reproduced the same failure
and retained `132880-timeout-1792275625280678.json` plus
`.execution-backend-fixture-132880-22`. Its exact deadline error, intact intent,
absent CID and native control-flow ordering identify the pre-create remaining
guard, not proof inferred from a missing fixture event or an absent name.
The old `submitted` flag was set before that guard and before native backend
entry. It therefore invented a possible create effect, performed an
unconfirmed name lookup and retained ownership after the required guard
correctly refused to accept absence as proof.

A deterministic control using a real owned Rust CLI and pre-plan lease failed
before the production repair with the same cleanup/deadline chain. The
control starts with an already expired 5000-ms controller deadline; it never
manufactures a passing observation. Native owned execution now supplies a
private submission witness whose only marking point is immediately before
the Windows spawn attempt. Validation/cancellation/deadline failures before
that point prove this create was not attempted. A missing PID, name or CID
after the marking point never grants that exemption. Existing uncertain
descendant ownership is not cleared.

The private state serializes bounded schema-one forensic receipts with the
native PID and attempt/spawn offsets from the production verification clock,
the pre-cleanup elapsed time and separate cleanup elapsed time. These appear
in `submission.json` and native report diagnostics; JSON is not ownership
authority or passing-test evidence. The ordinary completed observation also
rechecks the verification deadline before acceptance. The 5000-ms control,
ten-second cleanup budget, isolation flags and runner parallelism are unchanged.

Three added native cases exercise non-submission, rejected/cancelled native
entry and missing-PID spawn failure, plus pinned live Windows create processes
cancelled before an object or after an object but before CID publication.
The ambiguous case retains the writer without removal; positive recovery
requires actual matching ownership metadata and full-ID removal. Both keep a
foreign fixture object unchanged. These are Windows/fake-daemon proofs only.

The first repaired original-load run passed 369 library, six CLI, 53 backend
and two guidance cases, including the original Docker control, then failed
three configured local CLI cases with native main-stack overflows. The first
unchanged isolated CLI case reproduced that failure. Compacting the added
owned-future facade and using one common execution future removed the frame
inflation without a larger stack, different scheduler, detached work or CLI/
seat changes; all three unchanged targeted CLI cases subsequently passed.
That failed full campaign remains separate evidence, not a passing checkpoint.

The compact-source default stable campaign then passed all 483 cases. Its
matching MSRV full campaign exposed a distinct fixture precondition failure:
the original `rm` assertion failed after a live native receipt proved
`not-attempted`, no PID, and 5422.4542 ms before cleanup against the unchanged
5000-ms budget. No ambiguous cleanup was released; the required create/removal
action had never been exercised. Root `.execution-backend-fixture-79040-21`
and that failed campaign remain sealed evidence.

The owned Docker test now establishes executable/protocol readiness with one
bounded read-only image inspection in fixture setup, using isolated empty
configuration and the same physical approved fixture executable. Identity,
size/timestamp, actual image metadata, empty stderr, one read-only call and
zero objects are checked. Its separate at-most-30-second setup is not
verification evidence: the actual verifier still performs its complete fresh
image/create/inspect/start pipeline under the original 5000-ms timeout and
independent ten-second cleanup budget. Nothing is retried until fast or
silently skipped; every original action/outcome assertion remains. Setup PID,
elapsed time and call boundary are recorded separately in fixture diagnostics.

The final frozen-source original default campaigns each pass **483 native
all-target cases** on Windows GNU stable **1.99.1** and Rust **1.85.0**:
369 library, 53 backend, 53 local-delivery, six CLI and two guidance, zero
failed or ignored. Both use their matching installed fixture compiler and
Node 20.19.5 consumer approval. Formatting and strict locked/offline all-target
Clippy pass on both. Optimized CLI/local coverage passes **59 cases** and
standalone optimized help, exact argument JSON and invalid-option checks pass
with empty PATH. There is no new dependency, larger stack, altered scheduler,
changed original assertion or live Docker/model/GitHub/hardware workload.
The original parent failure, pre-fix native control, coupled stack failure and
actual non-submission fixture-precondition failure all remain retained;
these green component gates are not human acceptance or a completed rewrite.

## Fixed-profile outer local continuations

Product trace: FEATURE_SPEC **5.3 scoped expansion**, **5.4 bounded fresh
contexts**, **5.5 independent review repair**, 5.6 truthful metrics, 5.7 local
CLI and 5.8 ownership. Sections 3, 7 and 8 still bind this component. This is
not full rewrite/cutover acceptance or acceptance of the separate open vault
storage-ownership review; that source, contract and evidence are unchanged.

`delivery::Driver::run_local` now drives actual repeated `run_coder` and
`run_reviewer` calls in the same invocation/worktree/OS writer lease. Every
context creates a new seat chat, not a runtime, worker, branch or queue.
The caller token, whole-task deadline, credentials and shared `Admission`
are retained. The fixed selected model/profile is not silently switched.
No alternative-route eligibility or capacity is invented.

The independent axes each have a maximum of two transitions:

- A genuine completed semantic reviewer Fail with no terminal error may
  authorize a fresh coder, followed by a fresh read-only reviewer. Its private
  subject, artifact, context, coder checks and response-backed run must still
  be current. Malformed, unavailable, interrupted or erroneous reviews supply
  no repair authority. The review axis allows the initial coder plus two
  repairs. Passing review stops immediately. The same or a subset of prior
  unmet check IDs selects explicit changed-strategy guidance on the fixed
  model. Original Ask auditing remains independent and runs again, even when
  every numbered check was green.
- Only actual unprotected paths recorded by a scope-budget block permit
  rescoping. A positive budget below 16 becomes
  `min(16, max(current * 2, current + blocked_count + 1))`. Zero stays strict;
  protected/private/workflow/vendor paths never authorize a raise.
  Expansions, repair scope and own writes remain on the same boundary.
  Exhausted caps/aggregate scope produce `Status::PlanningEscalation` and
  `ROSTER_PLANNING_ESCALATION`, not success, ordinary task failure or automatic
  replanning. Correct source remains for an explicit later planning handoff.
- Actual turn, measured-context or test-repair exhaustion, repeated actual
  failures or bounded no-progress handoffs may authorize two fresh coder
  perspectives. A complete current context and privately bound stopped
  subject are required. Policy denial, cancellation, configuration/source/Git
  drift, unavailable capabilities, request-limit exhaustion and arbitrary
  exceptions are not perspective reasons.
  Exhausting a secret-content correction and a failed stop notification also
  block continuation; the private result carries that distinction without
  reinterpreting caller result JSON or replacing the original seat failure.

There are at most seven coder contexts across the three independent axes.
`Outcome.orchestration` carries `Counts`, bounded `SeatAttempt` records,
actual transition data and archive paths. Every attempt retains its actual
run, response/model/usage, outcome, handoff/error and applicable author
self-review or Original Ask audit response. Missing usage stays unknown.
These are diagnostics, not caller-constructible coder/review receipts.
Only `ReviewResult::require_passing` on the final current private subject can
produce Verified. Explicit skip-review, deterministic stub and planning-only
behavior retain their separate nonpublication contracts.

The coordinator captures its earliest source baseline once. Boundary own
writes, scope/repair paths, before-write checkpoint and failed-full obligations
accumulate across every context. `Boundary::continue_context` revokes prior
positive test receipts, required-read caches and context authority; it never
clears failed-full exits or imports passing receipts from disk. A scoped green
cannot erase those negative obligations. The host-only scope-budget guard
binds actual raises to the original frozen host/config intersection; a caller
starting with an overbroad boundary still fails preflight.

Native acceptance continuation is imported only from the current in-process
coder capsule, with unchanged TASK/source/worktree binding. All previous Done
closures and positive test observations are revoked. Actual unmet review IDs
are reopened with empty evidence and null refs; negative observations remain
negative. Neither required check text nor Original Ask authority is rewritten.
Every context receives complete required instructions/skills and current
continuation, rather than Node-specific repair prose for a Rust task.

Owned CONTEXT/RESEARCH/RESULT and, when present, the actual failed REVIEW are
archived with the existing Git-common archive. TASK, RECIPE, PLAN and ESTIMATE
are preserved; the coordinator additionally seals TASK/RECIPE/ESTIMATE physical
identities and bytes. Foreign replacements/arrivals, source/config changes
and archive failures stop explicitly. Post-archive checking accounts only for
the removal of the validated old context/research; a new file at either name
still fails. There is no source restore, unpublished-state inference or
automatic adoption of earlier on-disk runs.
Read-only archive preparation precedes one complete private-subject validation
immediately before effects. The post-archive check uses that original sealed
snapshot/Git/audit, not newly recaptured source or duplicated verification.

Actual `rescope`, `perspective-escalation`, `review-repair` and scope-expansion
events contain bounded redacted paths, counters, check IDs and measured models
where available, not raw transcripts or secrets. Scope records appear in
context, review, summaries and session logs. Ordinary session logging remains
minimal; durable recording/explicit promotion contracts are unchanged.

`llm.max_requests`, when present, also bounds actual sends across the entire
local invocation, including planning/critics, self-review, review and transport
retries. A fresh chat cannot reset it. An exhausted budget before transition
keeps the terminal feedback in place rather than archiving it for an impossible
next request. No token counts, prices or spend are inferred.

The delivery/seat/model-call/finalization entry futures use compact shared
facades so the actual configured Cargo CLI does not grow native main-stack
frames with nested continuations. No executor, thread, stack/resource flag,
dependency, backend/restore ownership or Docker readiness change is involved.
Source is sealed around each actual model request. Original context Git
validation is rechecked before any pending mutation, check/checklist or
completion, and before a stopped context may continue. Read-only exploration
does not redundantly recapture a new HEAD as retry authority.
See [TESTING](TESTING.md) for the writer-run matrix and retained first failures.
Independent component review is still required before further feature work.
The frozen writing handback is **validation-blocked**, not accepted: format and
strict all-target Clippy pass on stable 1.99.1 and Rust 1.85, but composed local
controls still exceed the unchanged existing 600-second owned HTTP workflow
guard in ordinary campaigns. Current results are stable local 88/93, MSRV
all-target 551/559 and optimized CLI/local 95/99. See TESTING for the retained
failures, unchanged expectations and precise source/compiler freeze. No
default full-green, release or cutover claim follows those partial results.

### Independent c5bc2474 Git-seal repair and bounded diagnostics

The frozen V3 source/evidence is preserved separately before this repair.
Four ordinary owned-Git controls first reproduced both high findings:
same-commit HEAD switching in the finalizing callback yielded Verified;
switching in the first per-tool callback allowed the second pending edit;
plain and linked-worktree index-only changes allowed the pending document
write and returned TurnBudget rather than PolicyDenied.

`Boundary` now retains one original private Git seal for the entire execution
boundary, not a newly admitted baseline for every context. It binds the existing
HEAD/branch/task/owner validation plus the actual worktree index and optional
split-index snapshot resolved through the existing effective Git command.
The existing `GIT_OPTIONAL_LOCKS=0` setup is unchanged; raw index identity/bytes
are checked without ignoring blob, mode, stage or flags. Owned controls verify
read-only Git does not refresh the index and a main-repository index change is
not mistaken for linked-worktree index drift.

Every effectful pending call rechecks that original seal, including the next
call after an awaited tool/scope callback. Completion rechecks after the awaited
finalizing observer and after evaluation, before returning a passing receipt.
Current coder authority, retained continuation subjects and post-transition
admission also require the same seal. Earlier state is not silently refreshed.
The six Git controls plus healthy docs and configured Cargo CLI controls pass
8/8 on stable and MSRV, with format and strict all-target Clippy passing.

Diagnostics add passive actual-send timing/counts and an error-only session
record of observer origin, phase, name and code before existing flattening.
They do not change returned errors, model/provider behavior, deadlines or
callback scheduling. Owned fixtures retain bounded monotonic server/client/
script snapshots outside product source and keep failed roots on guard errors,
returned errors or panic. No prompt, tool/source/key body or full error cause
is persisted in those snapshots. Server finish still fails on its original
guard instead of discarding evidence or converting it into a pass.

One unchanged ordinary local diagnostic campaign passed 93/100 and retained
seven failures. At every affected guard the actual client was pending, not
already returned: four had received their entire script and later returned
Verified; three had packets remaining and their first actual transport failure
occurred only after the server closed at its guard. No observer-origin error
was recorded for those seven runs. Exact roots, monotonic counters and times
are retained in `c5bc-actual-guard-correlation.json`. This identifies the
guard/client relationship, not the precise awaited native guard/verification
operation. No further full/release gate or default replay is claimed without
that narrower cause proof. Independent c5bc review is required again.

### Passive native-await timing checkpoint

Product trace: FEATURE_SPEC **5.4** measured native execution and **5.5**
independent verification; sections 3/7/8 remain unchanged. The independently
reviewed 148-file Git repair checkpoint and its failed default campaign are
preserved separately. This checkpoint adds timing, not a performance repair.

`Ownership::timings()` exposes a read-only `Timings` handle. Its bounded
`std::time::Instant` snapshot contains static phase labels, seat/context
ordinals, parent IDs, paired intervals, active awaits and aggregate counts/
inclusive totals/maxima. Direct-child interval unions provide exclusive
durations without counting nested awaits twice. The active limit is 64 and
the completed ring/child-interval limit is 128; eviction/truncation and unknown
exclusive durations are explicit. A scope-exit record is unclassified, not
invented cancellation. Mutation stays crate-private; no mutex is held across
an await and there is no new logging framework, configuration, dependency,
callback or runtime.

The owned fixture samples native state before reading the existing bounded
run log, recording the fixture/native clock bracket. No argument, path,
prompt, source, key or full cause enters native timing records. Only the
existing private fixture evidence contains its owned root/home. All original
Git/index, callback, source, authority and verification comparisons remain;
no check is cached, removed or reordered.

Four passive probes and the original nine Git/observer/healthy library/Cargo
CLI targets pass on stable 1.99.1 GNU; format and strict all-target Clippy pass.
The one authorized unchanged default local replay is **92/100**, eight
failures in 2076.11 seconds, with all 100 cases active. Five library guards
sample an owned Git await; one samples host review observations and one
samples failed-full classification. Across the seven instrumented guards,
100–188 completed owned Git awaits account for 147.461–268.871 seconds; actual
model awaits total only 0.453–0.786 seconds. The failed-full run additionally
spends 114.892 completed exclusive seconds in classification and is pending
in its next classification for 98.619 seconds at the guard. These are measured
awaited wall intervals, not claims about Git CPU, OS, load or network causes.

All eight clients are still pending at their unchanged guard. Scope and the
configured Cargo CLI have received every response and later return Verified;
the other six first fail transport only after guard expiry. There is no
observer-origin error. The separate CLI process has no live timing handle in
the fixture, so its pending internal operation is unknown. Preparation,
coordinator self-time and protected backend stamp/classification internals
remain coarser boundaries, not attributed suboperations.

Exact roots, counts, phases, intervals, return/error times and source/binary
identities are retained in `timing-guard-correlation.json`,
`timing-campaign-metadata.json` and `timing-diagnostic-source.json` under the
ignored outer-local evidence directory. The source freeze has 149 files;
the expected matrix is 570 cases (566 existing plus four timing probes),
not a completed green campaign. Writing stops at this diagnostic checkpoint.
No subsequent full, release or MSRV campaign, optimization, storage repair,
publication or cutover acceptance is claimed.

### Same-boundary query consolidation: historical query150 preflight

The reviewed timing149 source and evidence, plus the parent's updated Tokio
Git/index instructions, were preserved before this preflight. No production
query, parser, timing label, comparison, process setup or authority predicate
has changed. Product trace remains FEATURE_SPEC **5.4/5.5**.

Owned Git 2.51.2 probes show why ordinary two-line output is not sufficient:
strict abbreviation returns `heads/name` instead of the old branch value when
a branch and tag share a name. A plumbing-created `refs/tags/HEAD` makes the
bare full-ref query omit its second stdout line. A per-query
`core.warnAmbiguousRefs=false` recovers the committed branch case, but cannot
preserve the old subject when the actual current branch is absent: the old
queries return the HEAD-named tag's OID and the unborn branch name, while the
combined query reports `refs/tags/HEAD`. Status-v2 is not a proven substitute:
an ordinary branch named `(detached)` and detached HEAD at the same OID produce
identical branch/OID headers, although the old validator treats them differently.

A new ordinary native regression reproduces the authority discrepancy through
the existing `Boundary::admit_git` and `Boundary::require_git`, not a fabricated
receipt. It captures the original private seal on a genuine committed branch
with a HEAD-named tag, deletes only that branch ref using owned Git, and proves
the task, recipe and actual index bytes are unchanged. The actual branch ref
is then absent, but the old validation still equals the original and the same
original seal remains current. No fresh baseline is captured after the change.
The expected-refusal assertion fails on stable and MSRV; no coder/reviewer
passing result or Verified delivery was attempted.

Consolidation stops here rather than disguising an authority tightening as
equivalent query coalescing, guessing a branch or reusing a prior value.
Parent independent review confirmed that accepting that state violates the
existing committed-task-branch contract: it is an authority bug, not a product
design choice or a reason to amend the spec. The subsequent grant explicitly
authorized its correction and then query consolidation, described below.
The historical source, failed assertions, logs and roots remain preserved in
`query150-preserved`; they are not rewritten as passing evidence.

### Actual committed-branch authority and one owned subject query

Product trace: FEATURE_SPEC **3**, **5.4** and **5.5** require a real current
branch claim and default-deny authority. `checkpoints::validation` now obtains
the actual-current-ref marker, full object ID, direct object type and full
branch ref from one owned Git `for-each-ref` invocation. It prioritizes the
current-ref marker and bounds output to one candidate in `refs/heads/`.
Only an actual current marker, a commit object, a full SHA1/SHA256 ID and a
nonempty heads ref produce a subject. It never resolves a revision named HEAD,
so a HEAD-named tag, branch, remote ref or other ref cannot mask the disappearance
of the actual current branch. Detached and unborn states fail closed.

The standard-library parser bounds bytes, rejects invalid UTF-8, missing/extra
records or fields, noncurrent markers, noncommit types, invalid IDs and wrong
namespaces. Full-ref prefix removal and the existing trim preserve the exact
valid display previously returned by `git branch --show-current`, including
literal `(detached)`, HEAD, namespace-like names, Unicode and colliding tags.
No abbreviation guess, empty default, cross-await cache or positive reuse is
introduced. The same hardened Git image/environment, optional-locks setting,
ownership and process limits remain; the producer makes no repository/global
configuration change and needs no warning-disambiguation override.

The returned validation still has the same head/branch/taskSource/owner fields
and valid-state values. Original private Git/index seals, both index checks,
task/owner/source comparisons and every existing callback/effect/completion/
current-subject/continuation boundary are retained. The authorized tightening
is refusal of invalid actual committed-branch authority, not a refreshed
baseline after drift. The active regression now requires explicit refusal
from both validation and the original seal after deletion; the unchanged
task, recipe, actual index and product bytes, no-effect state and original
seal are still asserted. Valid coexisting HEAD tags remain allowed first.

Stable 1.99.1 GNU and Rust 1.85 GNU pass format, strict all-target Clippy,
**16/16** selected authority/equivalence/parser/passive controls and the
unchanged **9/9** Git/index/callback/observer/healthy library/Cargo CLI controls.
The unit counters prove two fresh validations execute two owned queries,
instead of the old four. A matching completed eight-response/two-rescope
workflow records 72 old HEAD plus 72 branch queries versus 72 current subject
queries; total owned Git calls are **191 to 119**, while original Git-seal
validation counts remain **37 to 37** and index checks **74 to 74**.
These are physical invocation counts, not an intrinsic Git/OS latency claim.

The single authorized unchanged default local replay is **97/100**, exit 101,
1617.63s, not green. Three original guard failures remain. Subset and two-repair
workflows receive all 15 responses before their guards and are then pending
in current end-to-end evidence checks; they return Verified and the intended
semantic Unverified outcome respectively, but still fail their server guard.
Failed-full is pending in classification, then first fails an actual transport
attempt after the listener guard. No observer-origin error is recorded.

The next measured boundaries are classification and end-to-end current-evidence
validation, not another authorization to cache/skip Source or Git checks.
At the failed-full guard, two completed classification calls account for
180.433526 exclusive seconds and the active third for 77.439757 seconds.
The two repair guards have 13/14 completed current-evidence checks totaling
115.802570/120.058589 seconds and active checks of 10.904280/1.390114 exclusive
seconds. Nested inclusive totals are not added again; internal protected
backend suboperations are not separately attributed.

`actual-head-default-source.json`, `actual-head-default-correlation.json`,
`actual-head-matched-scope-profile.json` and the handback in ignored outer-local
evidence retain exact source, roots, scripts, counts, clock intervals and
outcomes. The 150-file source has an expected **582-case** matrix, not a green
full campaign. No further default/full/release replay or broader optimization
follows this writing checkpoint. Fresh independent patch review is required;
default, cutover, full-rewrite and separate storage acceptance remain open.

### Deep passive proof-cost checkpoint

Product trace remains FEATURE_SPEC **5.4/5.5**, with default-deny and current
authority from section 3 intact. The reviewed actual-head150 source/evidence
and all three preceding failure timelines are preserved separately. This
checkpoint changes timing only, including the explicitly permitted passive
exception in `backend.rs`; owned/Docker/restore/LLM/vault engines remain untouched.

An execution `Owner` forwards static spans to the existing bounded `Timings`
clock. Existing operations are now separated into Stamp capture/current,
owner/image checks, source capture and subject comparison, Git operation
families and actual execute awaits, consumer setup/test/version calls, readonly
CLI calls and scratch creation/materialization/cleanup. Neither checks nor
commands are repeated to measure them. Original comparison short-circuiting,
pending-owner settlement/drop ordering, every await, source/index/ref guard,
receipt and error are retained. Pre-plan owners have no new timing handle;
their previously coarse preparation time is not falsely attributed.

The new actual Stamp probe permits current evidence, changes owned source,
then requires refusal. It observes three captures, twelve real Git signature
queries and no data/path bodies in bounded diagnostics. All 582 existing
conditions remain; this adds one probe, making **583 expected cases**.
Both GNU compilers pass format/strict Clippy, **17/17** selected unit controls,
the unchanged **9/9** delivery controls and **4/4** applicable classification/
end-to-end positive, sticky-negative and source/scope-refusal controls.

The smallest isolated failed-full selector passes **1/1** in 291.16s with
16/16/16 actual sends/accepts/responses, one rescope, two perspectives and
RepeatedFailure. Its ten classifications materialize ten bases (seven blobs
each) and perform forty fresh four-part Git signature inspections. Across
the whole invocation, 295 backend Git execute awaits account for 131.344s,
96 consumer execute awaits for 54.934s, while source/image checks are much
smaller. The isolated run never reaches its guard; it cannot establish which
inner await was pending at 600s in a different-load default campaign.

The conditional single unchanged default diagnostic replay is **98/100**,
exit 101, 1587.37s. Failed-full and subset still fail the original guard:

- Failed-full is in baseline classification's `show-ref` inspection, inside
  the actual backend Git execute await. At guard it has completed 79 such
  awaits totaling **243.178753s**, plus 27 consumer execute awaits totaling
  **63.879970s**. The active Git await is 0.006769s old; the enclosing third
  classification is 82.870488s old. This is accumulated physical work, not
  one 82-second metadata comparison.
- Subset has served all 15 responses and is in current end-to-end evidence,
  Stamp recapture's HEAD inspection, inside the backend Git execute await.
  That await is 1.014419s old. Its 88 completed backend Git awaits account for
  **222.241263s**. Source capture, image checks and subject equality are not
  the dominant measured work.

Failed-full first records a real transport error only +40.659s after guard,
then returns Unverified/Failed at +52.995s. Subset returns Verified at +30.494s
without a transport/observer error, but its guard failure remains a failure.
The former two-repair guard case completes before this run's guard with all
15 responses, two repairs and actual ReviewFailed. This does not explain its
earlier timing by load or make the full matrix green.

The unimplemented surgical proposal is to derive the HEAD component from the
same fresh `show-ref --head` observation already required by each Git signature,
rather than launch another `rev-parse` process for that component. The four
logical components, exact current-subject/source/owner checks, before/after
baseline proof and private receipts must remain. Prove byte/semantic parity,
HEAD-tag/linked/detached/SHA1/SHA256/error behavior and await-order safety before
adoption; no previous positive capture may be reused. Baseline blob batching
and Source/image caching are not implemented or authorized by these data.

Static spans retain the existing 64-active/128-completed/128-child limits.
Newly visible capped parent intervals have explicitly unknown exclusive time;
evicted pending-child end records are not invented. Known leaf intervals and
their parent chains still localize the actual awaits. The matching completed
scope profile retains **37** Git validations, **74** index checks, **72** current
subject queries and **119** boundary Git calls. No logical guarantee, ref
consolidation, guard or resource setting changed.

Exact frozen source and timing evidence are under ignored outer-local
`deep-proof-*` files. Writing stops after the attribution and proposal, before
any performance/semantic root fix. No second default/full/release campaign,
default/component/cutover acceptance or storage repair follows this checkpoint.

### Fresh four-part signature with three physical Git reads

The subsequent bounded patch was independently reviewed by c5bc with no
significant issue. `backend::git_signature` retains four ordered SHA256
components: HEAD stdout, staged-index diff stdout, the entire refs stdout and
local-config stdout. Its three fresh physical reads are index, required refs
including HEAD, and config. A small byte-only parser derives the exact HEAD
component from the unique literal HEAD pseudo-entry in that same refs result;
it never confuses a tag/branch/suffix/peeled name with that entry or decodes
all refs/config as UTF-8. Bounds, malformed/missing/duplicate HEAD refusal and
SHA1/SHA256 validation are explicit.

Actual old-four/new-three Git value parity covers main/linked roots, detached
captured baselines, colliding HEAD names and real non-UTF8 packed refs/config.
Source/index/config/ref drift and cancellation still revoke actual private
Stamp authority. No prior capture, positive receipt, context or await boundary
is a cache. The same owner checks surround each actual remaining await; source
and before/after baseline proof, consumer/setup calls and sticky negatives
remain unchanged. Only the directly coupled diagnostic physical-count
expectations changed from 12 to 9 for three fresh captures, with a derived-HEAD
label instead of a fabricated HEAD process.

Both compiler targeted gates passed 28 unit, nine delivery and four backend
controls. Focused F completed all 16 requests with RepeatedFailure and B/E all
15 each. A matching complete F trace had forty fresh signatures/twenty Stamp
captures and 295 to 255 backend Git inspections. These are physical counts,
not an intrinsic or load-controlled latency claim. The source151 default
campaign remained red at 93/100; its five guards and two process-start failures,
source and evidence are preserved in the parent-owned read-only archive.

### Candidate validation repair and remaining guard cost

Product trace remains FEATURE_SPEC **3**, **5.4** and **5.5**. The archived
source151, old failed roots and sibling homes are never modified. The two
start errors did not reproduce on unchanged source in their exact two-case
selector, the seven affected cases or subsequent stable/MSRV targeted gates.
Their historical OS cause was not retained and is not called load/transient
or falsely reported fixed.

`start::Git` now preserves bounded failure-only OS error/kind, cwd state,
existing image/cancellation-check stage and cleanup/PID facts as a cause,
without changing the primary error name/code or mapping ProcessStart to Stale.
Native controls observe actual Windows OS267/NotADirectory for missing and
file-valued cwd, while cancellation keeps its original reason without a spawn.
The two start controls retain their owned fixtures on returned errors as well
as panic, repairing the earlier missing-root forensic gap. Their original
STALE_SOURCE and exact-submodule assertions remain.

Passive start Git spans attribute previously coarse preparation/coordinator
work. Complete affected paths perform 61 start Git calls; 22 are the same
eleven-path ignore validation at two distinct boundaries. Each boundary now
uses one existing readonly Git call, requiring byte-exact complete coverage
of all eleven fixed paths. Git's successful exit for only some ignored paths
is not accepted. No ignore rule/policy is written, no path check is dropped
and no state is reused across the intervening await/boundary. Matching complete
paths now perform **41** start Git calls and two complete ignore observations.

Stable/MSRV pass format/strict all-target Clippy, **31/31** unit controls,
**11/11** delivery/start controls and **4/4** backend proof controls. All five
affected continuation selectors pass with their original HTTP counts and
terminal assertions. The one unchanged default100 replay remains
**96/100**, exit 101, 1697.61s. Both original start cases pass; four guards remain:
scope 3/6/12, Unicode review, failed-full and subset continuation. Three serve
their full scripts and later return Verified. Failed-full has 7/16 responses,
is pending in baseline tree inspection/backend Git execution and first fails
transport only after its guard. No observer-origin error is recorded.

At those guards start Git execute awaits already total 129.943–207.675s.
The next bounded, unimplemented proposal is to jointly read show-toplevel,
git-common-dir and git-dir inside each fresh `Repository::inspect`, retaining
all three identity comparisons and every later source/index/ref/config check.
No metadata, Source, image or positive/negative proof cache is proposed.
Broader baseline blob/stdin/pipeline work still requires separate approval.

The repaired source has 151 files and **597 expected cases**, including the
original 594 assertions and three additional library-only controls; local
remains exactly 100. Independent review subsequently cleared the
diagnostic/retention/ignore-batching source without accepting a historical OS
cause or a green default. Ten attested fresh retained containers, including four
failed guards, were hash-preserved by same-volume rename into the new ignored
forensic output area after quiescence; all sibling homes and the old external
archive remain untouched. No second default, full/release gate, publication,
default/component/cutover or separate storage acceptance follows this freeze.

### Same-inspection repository location reads

Product trace: FEATURE_SPEC **5.4** measured process-hop reduction and **5.5**
fresh, truthful proof. Each current `Repository::inspect` now obtains actual
show-toplevel, git-common-dir and git-dir from one existing owned Git
`rev-parse` call instead of three. All three native filesystem identities are
still captured before that call and compared afterward, with the original
relative-path resolution. Every later HEAD, branch, config, status, worktree,
source and current/private authority check remains in place.

The parser bounds the observation to the existing 16 MiB limit, requires
UTF-8 and exactly three complete nonempty LF-delimited records, refuses NUL,
and retains the old per-record CR/LF-end trimming without trimming spaces or
unquoting paths. Known native worktree/common/git-dir paths containing LF
select the original three-query implementation **before any query**. This is
a delimiter-compatibility precondition, not a retry after process, parsing or
identity failure; no new global path restriction or cached identity is used.
The bounded static timing label is `start.git.paths`.

Nine new Windows GNU controls prove raw-byte/value/identity parity for main
and linked SHA1/SHA256 repositories, spaces/single quotes/Unicode and relative
locations; detached/unborn behavior, actual Git/start/cancellation errors,
source/config/staged-index/ref revocation, hard-linked marker refusal, and
raw-frame bounds/encoding/precondition handling. SHA256 metadata parity does
not waive the existing full-inspection denial of `extensions.objectformat`.
The first new run's 7/9 result, source and both roots are retained: one fixture
required unavailable Windows symlink privilege (OS1314), and another had
incorrectly expected that existing config denial to succeed. Neither caused
a production-policy change. A Unix symlink control is added but unrun; actual
Unix LF-path execution and Windows symlink-creation proof are not claimed.

Stable/MSRV both pass format/strict all-target Clippy, **40/40** unit controls
(the original 31 plus nine), **11/11** delivery/start and **4/4** backend proof
controls. The Windows GNU candidate retains 151 source files and all original
597 cases, with **606 expected cases** now. No default100, full606 or release
campaign follows this patch; the previous source's default96/100 remains the
latest default observation. Independent source review later cleared this
path-query patch without accepting the default.
Baseline blob batching remains unimplemented: the existing owned transport
forces null stdin and has no finite-input/EOF interface, while its buffered
output limits cannot accommodate all currently valid 128 MiB baselines.

### Finite-input Stage A: verified provider-ownership stop

The path-query patch subsequently received a clear independent source review,
and the parent's unchanged nine-case path selector passed 9/9. This is not a
default, historical OS-cause, platform or whole-rewrite acceptance.

The next Stage A prototype added a private validated heap input capped at
256 KiB, one shared core, a pump owning `ChildStdin`, concurrent bounded drains,
direct original-handle leader waiting and the existing whole-job settlement.
It was not adopted. A first native TDD run was 1/2, followed by a 3/4 small
implementation run. The latter exposed premature input completion: on Windows,
`write_all` can acknowledge the complete 256 KiB before the underlying write
has finished. Adding flush correctly made the writer remain pending and the
direct leader-exit control then passed, without reopening a PID.

The decisive safe-public-API probe fails the required ownership condition on
both stable and MSRV: idle handle transfer preserves the handle; after a full
256 KiB write is acknowledged, flush remains pending and handle transfer must
duplicate the handle. Actual owned-job cleanup completes in both probes.
Tokio 1.53.3's Windows `ChildStdio` owns an `Arc<StdFile>` plus
`Blocking<ArcFile>`; its blocking write task owns another reference. Dropping
the public stdin object or transferring its handle does not cancel or join
that busy writer. `shutdown` is not a substitute for flush or cancellation.
Consequently this design cannot honestly satisfy closing **all** writer state
before job shutdown on cancellation/drop through the existing safe API.
Object-drop scalar witnesses alone are insufficient proof.

Per the explicit API-gap stop condition, all prototype code, thirteen proposed
controls, compiler/native counterproofs and first failed fixtures are preserved
as ignored forensic evidence. Only this attempt's active source changes were
reverted. Every one of the reviewed 151 native files again matches the
606-case path-query baseline byte-for-byte; no finite-input API is shipped.
Both compilers then pass format/strict all-target Clippy, 22 existing
NoInput/submission/death/cancellation controls, six CLI controls and the real
dropped-preplan-future writer-quarantine control. These validate restoration,
not Stage A. Remaining proposed input controls were not run to claim a green
primitive.

No unsafe/raw-handle construction, PID-based leader observation, dependency,
job-flag change, detached writer, alternate pipe provider or weaker cleanup
contract was introduced. A separately reviewed cancellation-owned pipe design
is needed before retrying Stage A. Git input routing and blob materialization
remain unimplemented; no default/full/release campaign or storage acceptance
follows this stop.

### Finite-input Stage A: retained writer and terminal join

Product trace: FEATURE_SPEC **3**, **5.4** and **5.5**. Independent review
confirmed the preceding counterproof. The parent then accepted the internal
retain-and-join ordering contract matching existing strict NoInput semantics,
not an alternate named-pipe provider or a claim of physical closure on Drop.
The old failed pre-kill closure assertion and its source/evidence remain
unchanged and failed; the new after-kill join control proves a different,
explicitly approved property.

`execute` and `execute_observed` keep their signatures and route None through
one shared core. None still installs null stdin. The new crate-private
`FiniteInput` owns at most 256 KiB of heap bytes, validates before the private
submission witness/spawn, and gives Some(empty) a real pipe closed for EOF.
There is no public model/configuration stdin option, new dependency or runtime.

The Windows input controller owns `ChildStdin`, input bytes and a checked cursor
outside transient select/try-join futures. Normal operation moves from Writing
to Joining, requires terminal error-free flush of the actual blocking worker,
then transfers and disposes the **idle original** handle for EOF before marking
input delivered. Native pointer-equality checks reject a duplicated busy
handle; no handle values or input data are logged. Successful command completion still
requires bounded output drains and the original actual whole-job settlement.
The separate leader wait uses the original Tokio process handle, never a
reopened PID or wrapper/group wait as leader authority.

Abort latches the first cancellation/timeout/overflow/leader/input error and
stops submitting new bytes while retaining stdin and its pending worker.
Owned job shutdown and terminal stdin flush/join are driven concurrently under
the **one existing five-second cleanup deadline**. Terminal broken-pipe error
may be physically joined but cannot become complete input success. Known
cleanup requires both actual job settlement and verified idle-original-handle
disposal; secondary pipe failure does not replace the first cause.

Unobserved future Drop, unknown cleanup or cleanup-deadline failure cannot
authorize release. There is no instant-FD-close or no-I/O-after-unpolled-Drop
claim. Tests use a Windows-test-only factory returning the real existing
`PendingOwner`, not a shadow lease: pending, unpolled and post-EOF abandonment
retain real writer quarantine; even an actual later terminal join cannot
unquarantine the abandoned result. No production Owner/Git input route exists.

Sixteen new controls cover these states, binary/NUL/max/empty EOF, full-duplex
backpressure, pending cancel/deadline/overflow before actual lease release,
early exit0, descendant-held pipes, whole-job waiting and typed uncertainty.
Stable/MSRV both pass format/strict all-target Clippy, **38/38** unit controls
(16 new plus the unchanged 22 NoInput/submission/death/cancellation controls),
**6/6** CLI controls and the existing real writer-quarantine drop control.
The current Windows GNU source has **153 files and 622 expected cases**
(461 library/53 backend/100 local/6 CLI/2 guidance). No original 606 assertion
is relaxed. Stage A is frozen for independent source review before any
Owner Git batch/materializer integration. No default/full/release campaign,
platform/cutover, pending human CI or separate storage acceptance follows.

### Immutable baseline blob batches

Product trace: FEATURE_SPEC **5.4** measured native process boundaries and
**5.5** unchanged independent proof. Stage B uses the independently reviewed
Stage A finite-input core without changing its source or settlement contract.
The parent corrected only Stage A handback metadata attribution to the parent
internal protocol; both the original and corrected metadata hashes are retained.
That is not a human acceptance or publishing grant.

An absolute approved Git image enables one narrow, crate-private
`Owner::git_blob_chunk`: fixed readonly `cat-file --batch`, full SHA1/SHA256
OID lines selected from the immutable tree, the existing hardened Git
environment, private submission witness, caller token and owner pre/post
checks. Unknown cleanup and abandoned futures quarantine the writer and
scratch. A relative legacy image selects the old per-blob implementation
**before** any batch attempt; it never falls back after an error or pretends
the relative `process::execute` path supports input.

The same tree observation uses `ls-tree -l` on the batch route. Pre-clone
retired-package refusal, clone/read-tree, path/mode/gitlink validation before
skips, and approved local submodule recursion remain. Only selected bodies
count toward 16 MiB per blob and **128 MiB per recursive materialize call**,
not a new global cap. Chunks preserve order and duplicates, capture at most
16 MiB of bodies plus 80 bytes per response and explicit 1 MiB stderr headroom
(maximum 17,989,632 bytes), and request at most 133,120 input bytes.
Legacy Git's 16 MiB default and owned transport's 32 MiB hard ceiling remain.

Every whole chunk must be known settled and fully validated before its
exclusive writes: ordinal full OID, exact blob type, checked canonical size
matching the plan, independently bounded header, exact raw body/LF and no
extra bytes. Binary bodies are borrowed ranges into one bounded buffer, not
UTF-8 or whole-baseline copies. Per-file current owner/token/path checks and
100644/100755 materialization remain. Real tests include valid 128 MiB,
oversized managed skips, duplicate OIDs/paths, source/index/ref/config/owner
drift, package/mode refusals, approved submodules, depth, actual cancellation,
overall-task timeout, overflow/error and no passing partial chunk writes.

A tightly coupled configured Cargo CLI stack overflow was retained and
diagnosed without stack/thread/timeout changes. New batch-future pinning alone
reduced its frame from 10,336 to 192 bytes but did not fix the CLI. Pinning the
actual existing Owner Git await facades then reduced the legacy Git frame
10,560 to 368 bytes and the materializer 11,232 to 1,040; the exact previously
failing CLI and both full target selectors pass. Routing, checks and Stage A
core semantics are unchanged.

Both stable/MSRV pass format/strict all-target Clippy, **75/75** unit controls
(16 new plus Stage A's broader 59), **6/6** CLI, **12/12** Git/start/quarantine
and **4/4** current/classification proof controls. Stable F and B/E and matching
MSRV F/B/E pass their unchanged actual scripts. Complete F retains 70 logical
blobs, ten baselines, twenty Stamp captures, forty fresh four-part signatures
and twenty consumer setup/test runs each, but uses ten batch processes instead
of seventy per-blob processes: **195 versus 255** backend Git processes in the
matched historical trace. This is a physical-count reduction, not a wall-time
causal claim.

The one unchanged default100 campaign is **98/100**, exit101, 1342.41s. F
reaches eight of sixteen responses at its unchanged 600-second guard, pending
in baseline `update-ref`; it later fails transport after the listener closes.
Subset review serves all fifteen responses, is pending in current e2e Stamp
cached-index diff, and later returns Verified but still fails its guard.
Neither eventual result waives the test. The source has **155 files and 638
expected Windows GNU cases**. It is frozen for fresh independent review with
both guards, human CI, full/release/platform gates and separate storage
acceptance open; no further replay or optimization follows this checkpoint.

## Native file vault and selected endpoint credentials

Product trace: FEATURE_SPEC **5.8** vault/operations, **5.2** endpoint
configuration, **5.4** native execution and **5.5** verification; sections
3/7/8 still apply. The reference is `HEAD:src/vault/file.mjs` and
`HEAD:src/lib/secrets.mjs` plus their original independent tests, read through
Git history. No JavaScript implementation, frozen checkout, Node oracle or
runtime fallback is introduced.

### API and persisted contract

[`FileVault`](../rust/vault.rs) exposes `new`, `from_environment`, `get`,
`set` and `list`. The default remains the machine vault
**`~/.roster/vault`**, not repository state, machine history or curated memory.
The native CLI passes its invocation environment: `USERPROFILE` on Windows,
`HOME` on Unix. An embedding without a home must supply an explicit directory
or receive an error; it never falls back to the current directory or the
embedding process's unrelated real-user home. There is no new public override.

Missing vault/entry reads return `None`, and a missing vault lists as `[]`,
without creating a directory, key, lock or marker. Existing owned legacy vault
objects are hardened before use; missing-entry reads do not load an unrelated
master key. `list` checks names and physical objects, not decrypted values,
and still lists valid entry names when the master key is missing.

Names share the transport's ASCII identifier validator: 1–64 characters,
letter/underscore first, then letters/digits/underscores. `GITHUB_APP_*` is
forbidden case-insensitively. Writes require nonempty UTF-8 text and reject
PEM private-key headers. Whitespace-only secrets remain nonempty, matching the
original resolver rather than silently trimming credentials.

The persistent format is unchanged:

- `.key`: exactly **32 binary CSPRNG bytes**.
- `<lowercase hex of UTF-8 name>.json`.
- Exact writer order/shape:
  `{"version":1,"iv":"BASE64","tag":"BASE64","ciphertext":"BASE64"}`.
- AES-256-GCM; random 12-byte nonce, 16-byte tag, UTF-8 name as AAD.
  Padded standard Base64 is written; legitimate unpadded fields, numeric
  version `1.0` and unknown envelope fields remain readable.

Malformed JSON/Base64, invalid dimensions/version/UTF-8, wrong key/AAD/tag,
corruption, permissions, links and changed identities are errors, never
absence. Noncanonical junk Base64 is not the permissive Node `Buffer`
decoder's recovery surface. Errors are typed, actionable, fixed and causeless:
they contain no actual path, entry name, key bytes, value or native error text.

These explicit input limits bound allocations, reads, scans and validation;
the original implementation had no explicit byte/entry limits:

| Input | Native limit |
| --- | --- |
| Secret UTF-8 bytes | **65,536**, inclusive |
| Envelope bytes | **87,640**, inclusive, including JSON whitespace |
| Entries inspected in a vault directory | **4,096**, inclusive, including ignored names and `.key` |
| Supplied and resolved path UTF-16 units | **32,766**, inclusive |
| Resolved path components | **256**, inclusive |

Paths must name a non-root directory. Blank-path validation shares the native
JavaScript-whitespace predicate, including BOM and line/paragraph separators,
rather than substituting Rust's different `str::trim` set.
Native filesystem limits can be stricter.
The Windows absolute-path API was measured separately: 32,766 units succeeds;
32,767 and 32,768 return native error 206. Exact threshold controls accompany
every explicit size/count limit. Limits are errors, not truncation.

### Native filesystem and key publication

[`vault/platform.rs`](../rust/vault/platform.rs) owns pinned operation handles,
identity checks, bounded reads and complete temporary writes. Both providers
reject Git ancestors, linked/reparse path components, nonregular and
multi-link files. The public vault does not reuse a path-name-only hardening
cache; recreated objects must pass fresh ownership and permissions checks.
Interfering path/file changes fail closed rather than being adopted.

[`vault/windows.rs`](../rust/vault/windows.rs) gets the **effective thread
token's SID** from safe native bindings. It never derives a principal from
`USERNAME`, environment account text, or a file's owner. Handle-bound
`GetSecurityInfo`/`SetSecurityInfo` verify ownership and enforce a protected
DACL with one current-user FullControl ACE, with directory child inheritance
only where appropriate. Foreign ownership is refused before ACL mutation.
Objects whose OS owner is not the effective user SID, including a group-default
owner in a privileged creation context, are not silently taken over; those
contexts require explicit operator ownership repair and remain unverified.
Ancestor/directory pins include directory read-data access and exclude delete
sharing; metadata-only access proved insufficient to prevent rename.
Files are single-link, no-follow, handle-bound objects. There is no production
PowerShell, `icacls`, helper process, unsafe application FFI or flag-mode ACL
claim. Independent fixture-only BCL observers audit the actual owner,
inheritance protection, ACE identity/count/type and FullControl mask.

[`vault/unix.rs`](../rust/vault/unix.rs) uses no-follow, directory-relative safe
`rustix` operations, actual effective UID ownership checks and handle-bound
`0700`/`0600` hardening. It has native mode/path-swap controls for execution on
a Unix host; Windows checks do **not** prove those controls. Unsupported
exclusive-rename platforms/filesystems fail explicitly.

Owned new empty directories are secured before this provider writes any key
material. Owned legacy vault hardening is deliberate compatibility, not a
relaxation of [STATE section 4](STATE.md): machine-history/state roots with
wider permissions are still refused. No existing state/settings privacy
provider was broadened or replaced; their Windows trusted-host limitations
remain separate.

A complete private temporary is flushed and closed before publication. The
master key uses an **exclusive rename**, not a hard-link publication:
Windows `MoveFileEx(WRITE_THROUGH)` without replacement, and supported Unix
`renameat_with(NOREPLACE)`. Thus readers never observe the old two-link
publication interval or a partially written key. Losers read and verify the
same complete winning key. Existing entries without a key never cause
regeneration. Entry replacement is atomic; failed writes retain the previous
complete entry, clean only identity-matched owned temporaries, or report
uncertain cleanup explicitly. A process crash can leave a private `.tmp-*`;
it is not adopted as a key or silently deleted on reads.

Raw master-key buffers use `Zeroizing`; explicit `aes`, `ghash` and `polyval`
feature unification clears expanded AES and GHASH/POLYVAL state on drop.
OS randomness is fallible; no infallible RNG generator, custom cipher,
PRNG or Base64 implementation is used. This is not a claim that every
compiler/register copy or physical storage/cache has been erased.

### Actual native local-run consumption

[`secrets::resolve_secret`](../rust/secrets.rs) is shared by native transport
credential maps and the delivery provider. An own nonempty selected environment
value wins **without touching the vault or requiring a home**. Unset or empty
text falls back. Non-string environment/vault values and empty returned vault
values are invalid. Every vault failure becomes exactly
`Unable to resolve the secret from the file vault.`, without the original cause.

[`delivery::native_invocation_credentials`](../rust/delivery.rs) lazily opens
the real file provider for the primary and the **effective** selected critic,
deduplicating inherited names. It snapshots only those named values, never
enumerating/decrypting other entries or reading App credentials. Selected-value
redaction still covers both maps and both endpoint/model forms, including
non-HTTP error chains. Existing stricter environment-only `credentials` and
`invocation_credentials` APIs remain available and retain their nonblank checks.
The compiled [`main`](../rust/main.rs) now calls the native provider before
local-run effects. It does not merely expose unused library code.

Owned local fixtures supply dedicated outside-Git `HOME` and `USERPROFILE`
before invocation. The original six leaf CLI controls do not enter credential
resolution. Native authenticated loopback checks require separate primary and
critic vault values, exercise empty-environment fallback through the real CLI,
verify the delivered change/review, keep actual served model and measured/
unknown usage evidence, and inspect recordings for selected-secret leakage.

### Evidence and remaining limits

The exact checked compiler/count matrix and retained failures are recorded in
[TESTING](TESTING.md). `.vault-evidence` holds ignored hashes, logs and the
frozen handback. Original execution sources, 483-case assertions, failed roots
and `.docker-480-evidence` remain protected; only additive fixture-home/module
glue changes their existing integration source files, with byte-exact reverse
projections recorded.

The pre-review frozen handback passed **523 all-target tests** on both native
Windows GNU stable **1.99.1** and Rust **1.85.0**, with zero failed/ignored:
402 library, 53 backend, 60 local, six CLI and two guidance cases. Both
compilers pass formatting and strict all-target Clippy. Optimized stable
CLI/local controls pass **66/66**, plus **33/33** targeted vault/resolver
library controls. The original 483 cases remain unchanged; 40 are added.
The matrix is source-frozen, serial and locked/offline, not a warm retry or
changed-load substitute for a failed run.

Windows GNU native ACLs, junctions, hard links, pinned-directory denial,
wide/null-DACL hardening, denied reads, concurrent readers/writers, crash
publication and actual atomic replacement failure are concrete controls,
not host-injected platform tests. The unprivileged token could not create a
foreign-owned filesystem fixture; the native-descriptor rejection test is
explicitly not cross-user filesystem or impersonation proof. Unix/macOS native
compilation/execution, Windows MSVC, privileged cross-user/impersonation,
file-symlink privilege cases, physical power loss and release distribution
remain unproven. No live model, GitHub, Docker or hardware campaign ran.

Public shell/onboarding and standalone vault CLI commands remain unsupported.
This component is a tested handback for independent review, not full-rewrite,
release, publishing, or FEATURE_SPEC section 8 acceptance.

Independent parent verification also passed that frozen 523-case baseline.
Review nevertheless found an unresolved identity gap between temporary-file
validation and name-based publication/cleanup. This concerns same-account
namespace replacement, not a demonstrated cross-user ACL bypass, and remains
an acceptance blocker. Cooperative locking is not proof against a replacement
that bypasses it.

A separate credential-map repair rejects non-object vault mappings inside the
lazy lookup, before any transport send, while preserving environment precedence
and legitimate optional absence. Its three added regressions and the complete
**69-case LLM subset** pass on stable and Rust 1.85, with formatting and strict
all-target Clippy. The 523-case baseline predates this repair; no subsequent
526-case full campaign is claimed.

## Remaining implementation

- Strict Unix owned-process backend and real platform containment tests.
- Rust inline-test counterfactual extraction without production overlays.
- Additional native CLI leaves and the remaining read-only acceptance families.
- Outer resume/publication adapters supplying current publication evidence,
  operator transaction-recovery UI and real approved Docker/platform validation.
- Automatic fleet recovery/routing, waves, acceptance/learning policy,
  resume/cleanup and GitHub/publication adapters.
- Shell, onboarding, remaining CLI commands and resource distribution.
- Remaining acceptance cases from the old suite, translated without weakening.

The original direct-child process API remains distinct from the strict owned
backend. A missing containment provider is not a passing receipt. Dependency
research/acquisition for a justified manifest change is distinct from an
unauthorized live model/hardware campaign.

Exact legacy Unicode source ordering and Windows change-time projection still
require trusted compatibility inputs where applicable. Native providers and
real platform tests remain necessary before complete cutover/release claims.

## Dependencies and release

### Owned-process dependency rationale

Execution backends require Windows process-tree ownership established before
the target can run, including descendants that outlive the original process.
The standard library and the existing safe `winsafe` surface do not supply
Job Object ownership. PID-tree termination and direct-child `wait` cannot
substitute for that capability, and application unsafe/FFI remains forbidden.

The active Windows-only providers are:

| Capability | Exact crate | License / declared MSRV | Cargo registry checksum |
| --- | --- | --- | --- |
| Suspended creation, assignment and resume | Watchexec `process-wrap =8.0.0` | Apache-2.0 OR MIT / 1.75 | `df785e1a8a4f462ab97abb49e1718d8ee173f4fc37c29dde7d0487b6538c3a82` |
| Independent kernel Job membership/accounting | Ohad Ravid `win32job =2.0.2` | MIT OR Apache-2.0 / not declared | `15165faeb072f2dc68597de0d7d2071fa8ace463ec85b4a86063c0100c33bcc1` |

Startup and accounting are separate capabilities: Rust 1.85 cannot safely
expose a child's suspended main-thread handle, while the existing safe Win32
surface lacks `OpenThread`. `CreationFlags` precedes `JobObject`; the
`KillOnDrop` shim, not the underlying Tokio option, owns Job shutdown.
Only `tokio1`, `job-object`, `creation-flags` and `kill-on-drop` are enabled
for process-wrap; defaults, downcasting, Unix groups/sessions and the standard
process wrapper are disabled. The second provider uses Windows 0.52 and
thiserror 1; startup uses Windows 0.56. These duplicate binding families are
an explicit cost of safe startup plus independently checked ownership.

Filtered locked Cargo metadata for `x86_64-pc-windows-gnu` resolves **44 package
versions in the two-provider dependency closure**, including the providers and
dependencies already shared with the existing runtime, not 44 new packages.
The selected families include futures, indexmap/hashbrown, tracing and the
Windows bindings. All declared compiler floors are at most 1.85; win32job's
undeclared floor is checked by actual locked Rust 1.85 compilation and tests.
Licenses offer MIT/Apache-2.0-compatible choices, with the existing
Unicode-3.0 condition on unicode-ident. The transitive futures-executor crate
is present but is not invoked; the application still uses its existing
current-thread Tokio runtime. No blocking wrapper wait or detached drain is
used.

The published safe startup/error paths were inspected after the documented
manifest pins and official registry acquisition. Ordinary verification is
locked/offline. This dependency exception does not authorize live workloads.

Rejected investigations are retained as limitations, not adopted guarantees:

- The initial `process-wrap 8.2.1` candidate (MSRV 1.77, Apache-2.0 OR MIT,
  checksum `a3ef4f2f0422f23a82ec9f628ea2acd12871c81a9362b02c43c1aa86acfc3ba1`)
  faulted in the Windows GNU/MSRV test executable before even listing tests
  (`STATUS_ACCESS_VIOLATION`). Its wait cache and completion-packet handling,
  also present in inspected 8.2.2 source, are not completion proof.
- A real 8.0.0 wrapper-wait control returned while a detached descendant was
  still alive. Production never calls that wrapper's `wait`/`try_wait`.
- Kernel membership alone was insufficient in a real shutdown control.
  Independent membership plus signalled pinned process handles replaced it;
  root status by itself is never a descendant receipt.
- Newer 9/10 releases require compilers above the supported floor and are not
  selected. Windows MSVC, Unix execution and release distribution remain
  separate platform gates.

The [manifest](../Cargo.toml) and [lockfile](../Cargo.lock) pin the graph:
`fs4` for advisory locks; `reqwest` for maintained HTTP/TLS/compression;
`tokio` for owned asynchronous I/O; `serde_json` raw values and `ryu-js` for the
lossless/JavaScript-compatible codec; `regex-lite` for bounded grammars;
`sha1` only for legacy directory keys; `sha2` for integrity; `time` for UTC
formats; `url` for remote normalization; `winapi-util`/`winsafe` for safe
Windows file identity/liveness; and `rustix` for Unix process/filesystem
primitives. The vault's crypto, zeroization, encoding, effective-token and
handle-ACL dependencies are justified and pinned in
[DEPENDENCY](DEPENDENCY.md#native-vault-capability-dependencies), including the
BSD-3-Clause `subtle` condition. Rust's standard
library does not supply these capabilities safely and equivalently.

New crates require a concrete missing capability, license and MSRV review,
explicit manifest change and locked/checksummed resolution. Ordinary checks
are offline. No new executor, orchestration framework or npm runtime is needed.

Policy, workflows, release and binary distribution remain human-owned.
The owned Windows process controls do not prove Linux/macOS, ACLs, a complete
filesystem/network sandbox or mixed-version locking. No whole-runtime speedup
or harness acceptance is claimed. The frozen live campaign remains
independently unaccepted.
