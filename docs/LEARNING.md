# Learning from runs and human evaluations

Native Roster joins local Git `AI-Run` evidence with bounded run
metadata and human decisions. The goal is the **next task at this difficulty
with fewer defects**, not a hidden quality score. There is no model API, vault,
analytics service, or separate task database. GitHub Issues and PRs remain the queue.

## Native scope

`stats` and `recommend` are implemented read-only native CLI commands.
[`learning`](../rust/learning.rs) and [`metrics`](../rust/metrics.rs) retain
correction/deduplication, defect, unknown-usage, median and ceiling contracts;
the actual CLI uses approved native Git, not the retired Node exporter.
See [native metrics](METRICS.md#native-stats-and-recommendations).

Explicit human `eval ... --local` recording and opt-in `--post` feedback are
implemented separately in [`evaluation`](../rust/evaluation.rs).
Recurring proposal writes, automatic fleet routing and the historical delivery producer
remain unavailable. An agent/App never generates a human decision. Existing
human JSONL can be validated and summarized; no raw history is promoted into
curated memory.

## Historical automatic seat recording (native producer not integrated)

The automatic producer described below is not wired to the native delivery
path. Explicit human local evaluations do not manufacture a seat run, response
usage, verification result or publication receipt.

For issue runs, each completed planner, coder, or reviewer seat automatically creates
and appends to `.roster/runs/runs.jsonl` in the issue repository. Stub seats record session,
task, and a recognized task class, but no model or LLM counts. Configured
seats include model, effort, and token counts only when known. If recording
fails, the run reports the error instead of claiming a completed seat. A
coder failure can leave a planner record. Coder records retain the excellence
outcome and redacted failure reasons as `defects`, including configured failures
with result evidence, before the error is propagated. Publication rechecks
append new defects if the verified worktree changes. A recorded failure is
never treated as a human acceptance.
Reviewer records use a distinct `roster-N-reviewer` session and only
model/usage fields from an actual reviewer request. Its pass/fail verdict is
in REVIEW.md, not a human AI-Eval; a stub or incomplete review does not
invent usage or acceptance.

Manual `roster prepare --issue N` and the standalone
`roster run --seat coder --runtime builtin` remain opt-in. Create `.roster/runs` in
the repository root before preparing an assignment, for example in PowerShell:

```powershell
New-Item -ItemType Directory -Force .roster\runs
roster prepare --issue 42
```

After successful worktree and assignment setup, `roster prepare --issue N`
appends one JSONL record to `.roster/runs/runs.jsonl`. Without the directory
it records nothing and does not create `.roster`. Failed setup never records a successful run.
Recording failures are reported explicitly, including the already-created
worktree path; inspect that worktree rather than blindly rerunning setup.

Live `.log` files are always written by builtin runs, independently of this
JSONL opt-in. A standalone run records JSONL only when the runs directory was
already empty (explicit opt-in), or contained JSONL records, before it started.
A directory containing only automatically created logs does not enable
standalone metric recording. Stats and recommendations ignore those logs.

For the default `roster run --issue N`, setup adds no preparation record.
It records one run per completed seat (`roster-N-planner`, then
`roster-N-coder`, then `roster-N-reviewer`) with only that seat's reported metrics.

The prepare command assigns a worker, not a completed commit. Its generated
`session` and `task` are known, but the eventual commit SHA is not: the starting
HEAD is **not** recorded as the result. Published contracts metadata supplies
the SHA later, joined by session and, when available, task.

Only reported fields are written:

| Field | Source |
| --- | --- |
| `sha` | Full commit SHA, when known in a local run record or Git export |
| `session`, `task` | Generated worker assignment; contracts `AI_SESSION`, `AI_TASK` |
| `provider` | Actual configured or supplied backend: `vllm` for the named vLLM profile, `github-copilot` for an explicitly identified Copilot endpoint. The pinned contracts AI-Run uses `local` for vLLM. |
| `model` | Live seat's last response model, falling back to its request model; `AI_MODEL` for legacy/manual records |
| `effort` | Seat configuration, or contracts-normalized `AI_EFFORT` for legacy/manual records |
| `prompt_tokens`, `completion_tokens` | Live seat's last response usage, omitted when unreported; never accumulated or estimated |
| `context_used`, `context_out` | Aliases for live prompt/completion counts; corresponding `AI_CONTEXT_*` values for legacy/manual records |
| `context_max` | Selected fleet profile or seat configuration; `AI_CONTEXT_MAX` only for legacy/manual records |
| `task_class` | A recognized `feat`, `fix`, `docs`, or `test` prefix in the issue title |
| `excellence` | A recorded `pass`/`fail` or report with boolean `pass`; never a human evaluation |
| `defects` | Redacted excellence failure reasons; an empty array means no recorded gate defects, not human acceptance |

Run metadata normalization uses the contracts parser through
[`resolveContractsPath`](../src/lib/paths.mjs), not a copied parser.
Model-free journals normalize only partial effort and count fields locally;
they do not fabricate a model to serialize a contracts AI-Run.
Unknown fields are omitted, not filled with zero. A genuinely reported zero is
preserved. Live token counts must be nonnegative safe integers; legacy
contracts-sourced counts beyond JavaScript's safe integer range remain decimal
strings. Completed seat records do not inherit Copilot model or context values;
see [metrics](METRICS.md).
No environment secrets, issue bodies, prompts, or traces are recorded.

Example with only the assignment known:

```json
{"session":"roster-20260928T120000000Z","task":"issue-42"}
```

Local learning reads all `*.jsonl` files directly inside `.roster/runs`, in
filename order. Records need a full SHA or opaque session identifier; other
fields may be omitted. Matching records are combined rather than counted
twice. Known Git-exported values take precedence over local values.
SHA-specific corrections do not replace another commit's applicable
session-only decision. The normalized inputs retain the existing combined
32 MiB allowance; live joined rows have a 16 MiB compact-JSONL byte budget,
including local-metadata and evaluation expansion. The join checks replacements and
appended evaluations incrementally instead of materializing unbounded fan-out.
Defects are combined without duplicates across matching records, and failure
evidence cannot be cleared by a later passing report or evaluation. In particular,
a secret-path or policy touch remains a reject for recommendations even if a
human later records `accept`. The human evaluation itself is not rewritten.
Passing the gate still requires a human `roster eval` to establish acceptance.

## Historical recurring failure proposals (native command unavailable)

Run `roster learn --recurring` (or `/learn --recurring`) to inspect local run
and human-evaluation history without contacting a model or GitHub. The pure
`failureSignatures(records)` helper groups defects by gate name and normalized
reason: quoted identifiers, named exports, camel/snake-case symbols, file paths, and numeric
values become placeholders. Different gate names and materially different
reason text remain separate. Three distinct slice/seat identities are required;
repeated snapshots, repairs, or multiple candidates for one slice do not inflate
the threshold. Rejected/reworked human comments can supply a review signature;
latest human corrections retain their existing precedence.

The command writes `.roster/proposals/<gate>-<signature-hash>.md` containing
the normalized failure, cited issue/task/session/model/seat occurrences, a
proposed evidence-check rule, and a skill or principal target. Policy/scope/
secret/permission failures target a principal; other failures target a skill.
These are **drafts for human review**, not active instructions. History text is
redacted and escaped, and never executed. Unknown model or issue values remain
unknown. This traces to FEATURE_SPEC section 5.6 and lifecycle step 12.

Existing drafts, including human edits, are preserved on reruns rather than
overwritten. Their evidence is a snapshot at creation, not a live report.
Malformed history, unsafe paths, symlinks, and hard-linked drafts fail explicitly.
No proposals directory is created when no signature meets the threshold.
`roster stats` and `/stats` list pending draft filenames; an empty proposal list
does not change their existing output.

Review the cited failures, refine the proposed rule, then **manually** promote
it into the appropriate skill packet or principal note using the repository's
customization format. Move promoted or rejected drafts out of the proposals
directory. The command never edits `skills/`, `principals/`, routing, policy,
workflows, or the fleet, and cannot grant permissions. A gate failure or
proposal is never fabricated into a human acceptance/rejection evaluation.

## Native human evaluation

After reviewing the result, a **human** runs:

```sh
roster eval roster-20260928T120000000Z accept 3 n --local --minutes 18 --comment "Ship quality."
roster eval 0123456789abcdef0123456789abcdef01234567 rework 4 y --local --minutes 35
roster eval 0123456789abcdef0123456789abcdef01234567 accept 3 y --post --minutes 18
```

Exactly one of `--local` or `--post` is required. Local mode never invokes
GitHub or reads its credentials; neither mode invokes the App publisher or
changes policy. App credentials, `AI_*` declarations and `ROSTER_AGENT_SEAT`, including
case aliases and blank values, refuse the write before repository access.
Unmarked environment variables are not proof of human identity; only the
explicit human command supplies a verdict. Evaluation rows remain data, not
tool or publication authority. Roster's read-only acceptance catalog rejects
this mutator before execution.

The positional values are:

1. A full commit SHA, an unambiguous abbreviated SHA, or an opaque session.
2. `accept`, `reject`, or `rework`.
3. Human-rated task difficulty, an integer from **1 to 5**.
4. Whether the human would use that configuration again: `y` or `n`.

Full SHAs can contain 40 or 64 hex digits. Other all-hex identifiers of 4-63
characters are resolved through Git as commit abbreviations; use a non-hex
session such as `roster-...` to avoid that ambiguity. Sessions use the contracts
1-64 character alphabet (letters, digits, `.`, `_`, `-`), without a leading `-`
for CLI arguments. A full SHA or session need not already have a local run;
unmatched evaluations remain saved. Self-contained evaluations with a known
model and task class can supply local learning evidence without a separate run
record; incomplete metadata cannot establish model capacity.

This appends the canonical repository's shared evaluation ledger, even from a
subdirectory or linked worktree. Native layout uses
`.roster-state/shared/evals.jsonl`; the existing layout resolver retains
`.roster/evals.jsonl` when that legacy layout is selected. Recording neither
migrates nor copies another layout. The row format is:

```json
{"sha":null,"session":"roster-20260928T120000000Z","model":null,"task_class":null,"verdict":"accept","difficulty":3,"again":false,"minutes":18,"comment":"Ship quality.","at":"2026-09-29T12:00:00.000Z"}
```

Corrections append another record, preserving the history. The last evaluation
for an identical SHA or session wins; SHA matching is case-insensitive. A
SHA-specific evaluation takes precedence over a session-wide evaluation.
Malformed arguments or existing JSONL fail explicitly rather than silently
skipping data. Conflicting model, effort, task, task-class or seat metadata
for a local/Git session requires an explicit SHA instead of silently choosing
one worker. Private paths, endpoint URLs and secret-like model attribution
are refused, including attribution copied from earlier human rows.

The writer uses the existing cooperative ownership lock and checked atomic
replacement, preserves earlier JSONL bytes, and rechecks scope and file
identity before writing. A real single-link regular file, bounded UTF-8,
1 MiB rows, a 16 MiB ledger and at most 10,000 rows are required. Corrections
append rather than erase history; zero minutes remain zero and missing
minutes remain unknown. Comments are redacted data, not executable skills.
Cancellation after replacement is not rollback; inspect the ledger before
retrying an uncertain operation.

The human command enriches known SHA/session/model/class fields from run history.
If an opaque session has no matching local/Git metric record, it matches exact
durable session IDs or recorded operational attempt lineage through the
repository-scoped typed durable API at the machine
root selected explicitly by `ROSTER_STATE_HOME`. No default operator home is
discovered. This restores observed served-model, seat and task-class attribution
after a checkout loses its local ledger, including a reclone with the same
origin identity. Lifecycle records are not seat attribution. Multiple matching
session records, including retries of one operational session, fail before
writing rather than selecting an arbitrary occurrence. A specific
attempt-suffixed durable session ID can disambiguate that occurrence.
Local/Git matches retain precedence without a durable read.

Recovered decisions use the exact matched attempt session and retain a bounded
`provenance` reference with the source record ID, run ID and session ID.
An absent served model remains unknown;
the requested model is never substituted. A starting `repository.commit`
is never treated as a published evaluation SHA. Other repositories are
excluded by the existing identity-scoped typed query. Durable-history opt-out
skips the lookup; no origin gives an explicit local-only diagnostic, while
other reader failures stop evaluation before writing.

This lookup does not generate verdicts, migrate storage, write evaluations to
machine history or replace routing's existing ledger. Only a genuine human
decision is enriched. It implements [FEATURE_SPEC sections 5.6 and 5.8](FEATURE_SPEC.md).

### Explicit human GitHub feedback

`--post` requires `ROSTER_GH` naming an absolute approved GitHub CLI image and
an explicit human `GH_TOKEN` or `GITHUB_TOKEN`. The selected token is pinned
for the request sequence; no credential is taken from an operator auth store.
GitHub's actual `/user` response must identify a human `User`, not an App or
bot. An environment declaration or model name is not that principal proof.

Only a known full evaluation SHA is used to query commit-associated PRs in
the current credential-free GitHub origin. A missing SHA or an empty complete
association is reported as **not posted**, never as a successful comment.
Multiple PRs, foreign repository results, malformed data and failed identity
checks refuse the operation before saving a decision or posting. A specific
PR is not guessed from the current HEAD or a requested model.

After preflight, the local decision is saved and the origin is checked again
before posting only `AI-Eval: 1|accept|3|y` and optional `Minutes: 18`.
Zero minutes are included; missing minutes are omitted. The free-text comment,
model context and secrets never enter that request. The actual acknowledgement
must match the sent body and contain a valid comment ID before reporting a
verified PR URL.

Local persistence and remote comments are not one transaction. A failure or
cancellation after saving states that the local row exists and the remote
outcome may be uncertain. Nothing automatically retries; inspect the PR
before trying again. Unknown process cleanup or a dropped pending submission
retains the writer lock instead of claiming release. See the
[retrospective guide](RETRO.md) for verdict meanings.

Roster does not fetch PR comments, generate decisions, or write evaluation
trailers. The coder `run` and contracts publishing paths never write `AI-Eval`;
only the explicit human `eval` command writes the local evaluation file. Agent
seat calls and coder-tool writes to the evaluation file are rejected.

## Stats and recommendations

```sh
roster stats
roster stats --json
roster stats --ref main..HEAD --evals evals.jsonl
roster recommend --task-class fix --difficulty 4
```

Stats reads real Git objects/trailers natively and joins the local records,
including self-contained human evaluations when no exported run matches.
Rows are grouped by **model, task class, and effort**, so different effort
configurations are not mixed. The table retains `MODEL`, `EFFORT`, `RUNS`,
and `EVALS` and adds `TASK_CLASS`, `N`, `ACCEPT`, `MEDIAN_MIN`, and
`MEDIAN_DIFFICULTY`. Unknown values display as `-`, never invented zero actuals.
Medians use available human measurements across accepted, rejected, and reworked
samples. `N` counts distinct decisions, not duplicate exported commits.

Without `--ref`, stats includes exported `HEAD` history and local-only runs.
An explicit `--ref` restricts results to exported commits in that revision or
range; local records can enrich matching commits but cannot add out-of-range
or unpublished runs. The existing `--evals PATH` remains supported as an
additional legacy SHA-keyed evaluation file, resolved relative to the caller's
directory. Its SHA evaluations override matching local SHA evaluations. See
[metrics](METRICS.md) for the legacy schema and duplicate validation.

Evidence statistics and the internal learning helper group by
**task class, model, and effort**. Classification uses
`task_class` when present, otherwise a conventional task prefix such as
`feat-auth`, `fix.cli`, `docs-guide`, or `test-unit`. Issue titles such as
`feat(cli): add a flag` supply the class when roster records the assignment.
Opaque tasks like `issue-42` are not guessed into a class. Unknown models and
the deterministic `builtin-stub` are not candidates.

For each configuration:

- `n` counts distinct human-evaluated samples of the requested task class,
  plus recorded excellence failures that must count as rejects.
- Accept-rate is `accept / n`; `reject` and `rework` are not accepts.
- Unevaluated runs do not enter the denominator unless excellence recorded a
  failure. A passing test or excellence report never invents human acceptance.
- One session evaluation counts at most once per configuration, even when
  several exported commits share that session.

`--difficulty 1-5` requires median human-rated difficulty at least that high,
in addition to `n >= 3`. Missing difficulty cannot satisfy that request.
Eligible configurations rank by accept-rate, then sample count, then stable
model/effort ordering. The internal helper retains its existing optional
difficulty filter. Output includes the supporting medians; a candidate
also carries the rounded median **accepted** minutes for the next task estimate.
Missing timing remains unknown. If no candidate qualifies, the command prints
`insufficient data` and the actual config model/effort default (`ROSTER_MODEL`
when config has no model); it does not
silently choose a different model.

Security misses such as a secret in the diff or a policy edit count as
**reject**, even when tests passed or a human evaluation said accept, if an
excellence failure was recorded. This is a derived learning decision only:
the human's append-only AI-Eval is not rewritten. Duplicate run records cannot
erase a recorded failure. Failure-only evidence has no invented difficulty or
duration. Legacy free-form verdicts remain visible in `EVALS` but do not enter
the normalized `N` or recommendations. `again` and token context are evidence,
not extra ranking weights; difficulty is an explicit eligibility gate.

The native advisory command requires **three distinct human evaluations**,
not three automatic failure/pass records, for the coder seat. The requested
median difficulty and earned coder/model ceiling must both be supported.
Its default requested difficulty is two. It does not bind a fleet profile,
consult benchmark priors, validate a live endpoint or change routing.

Example output:

```text
feat: careful-model effort=h accept-rate=66.7% n=3 median-min=20 median-difficulty=4
```

If no configuration qualifies, output includes `insufficient data` and the
configured fallback as information, not an automatic fleet choice.
`roster recommend` is read-only. The historical registered-profile selector
and `roster run --issue N --runtime builtin --auto-model` remain separate,
unavailable native work. In the legacy contract that flag permits
a run-scoped endpoint/model choice without clearing or editing the saved
default. Qualified human data outranks priors; absent both, the run stays a
deterministic stub. An explicit task model outside fleet routing can select the coder as
described in [estimation](ESTIMATION.md). The [next planner task](NEXT.md) also
uses qualifying capacity evidence and carries forward redacted human feedback.
One acceptance can seed an otherwise unconfigured baseline, not a recommendation.
See [routing](ROUTING.md).

## Derived difficulty ceilings and skill notes (FEATURE_SPEC 5.6)

`learning::derive_difficulty_ceilings` reads joined human evaluations without writing a
new store. Each **seat + model** starts at ceiling **2**. Three consecutive
clean accepts at exactly the current ceiling raise it by one (maximum 5);
each reject lowers it by one (minimum 1) and resets the streak. Off-level
accepts and `rework` (accepted with defects) reset the streak and never raise
it. Recorded excellence defects turn an evaluated acceptance into a derived
reject, without changing the human verdict. Automatic-only runs do not
establish ceiling evidence.

Decisions are deduplicated by evaluation SHA (case-insensitive) or session,
across task classes and efforts. Existing join rules retain the latest human
correction and failure evidence. Complete evaluation timelines are ordered by
human `at` timestamps, with ledger order breaking ties; timelines missing any
timestamps use ledger order. Joined rows carry a derived, bounded
`evaluation_order` for this replay, not an invented human timestamp or policy
grant. Raw prejoined API rows without that ordinal retain caller order.
Explicit `seat` metadata wins, otherwise a session ending in `-planner`,
`-coder`, or `-reviewer` identifies the seat. Older unlabelled runs use the
coder bucket; unknown/stub models do not establish a ceiling.

Each derived reject adds a skill note containing the human comment, redacted
with the existing feedback redactor and blockquoted as **data, not a policy
grant**. Missing comments are stated, not invented. Notes are derived from
the existing local ledger, not written into executable skills or a new DB.
Native recommendations check the coder/model ceiling without assigning a
worker. Historical fleet routing checks the requested seat (coder by default) ceiling in both
evidence and prior tiers; no evaluations for that seat/model preserve prior
routing behavior. Existing median difficulty, context, and sample thresholds
still apply. Ceiling evidence does not pool another seat's performance.

Both `.roster/runs/` and `.roster/evals.jsonl` are ignored by this repository.
When running Roster against another repository, ignore these paths there
before recording seats or evaluations. Keep human free-text feedback local;
the human eval command posts only the compact verdict and reported minutes
when a matching PR exists. These files are evidence, not a task queue.

## Tests

Native fixture tests cover omitted unknowns, real zeros, joins and
deduplication, existing human evaluation input, actual stats/recommend flags,
the three-sample threshold, per-seat ceilings and read-only authority:

```powershell
cargo test --lib --locked --offline learning::
cargo test --lib --locked --offline metrics::
cargo test --test analytics --locked --offline
```
