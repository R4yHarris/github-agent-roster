# Learning from runs and human evaluations

Roster joins the required contracts pack's `AI-Run` export with local run
metadata and human decisions. The goal is the **next task at this difficulty
with fewer defects**, not a hidden quality score. There is no model API, vault,
analytics service, or separate task database. GitHub Issues and PRs remain the queue.

## Automatic seat runs and optional preparation records

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
Defects are combined without duplicates across matching records, and failure
evidence cannot be cleared by a later passing report or evaluation. In particular,
a secret-path or policy touch remains a reject for recommendations even if a
human later records `accept`. The human evaluation itself is not rewritten.
Passing the gate still requires a human `roster eval` to establish acceptance.

## Human evaluation

After reviewing the result, a **human** runs:

```sh
roster eval roster-20260928T120000000Z accept 3 n --minutes 18 --comment "Ship quality."
roster eval 0123456789abcdef0123456789abcdef01234567 rework 4 y --minutes 35
```

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

This appends `.roster/evals.jsonl` at the current Git repository's root, even
when invoked from a subdirectory:

```json
{"sha":null,"session":"roster-20260928T120000000Z","model":null,"task_class":null,"verdict":"accept","difficulty":3,"again":false,"minutes":18,"comment":"Ship quality.","at":"2026-09-29T12:00:00.000Z"}
```

Corrections append another record, preserving the history. The last evaluation
for an identical SHA or session wins; SHA matching is case-insensitive. A
SHA-specific evaluation takes precedence over a session-wide evaluation.
Malformed arguments or existing JSONL fail explicitly rather than silently
skipping data.

The human command enriches known SHA/session/model/class fields from run history.
With `gh` and one matching PR, it posts `AI-Eval: 1|accept|3|n` and `Minutes: 18`
using the human's GitHub identity, not the App. The free-text comment stays local.
Missing minutes remain unknown (`null`); legacy four-argument calls still work.
See the [retrospective guide](RETRO.md) for verdict meanings and failure handling.

Roster does not fetch PR comments, generate decisions, or write evaluation
trailers. The coder `run` and contracts publishing paths never write `AI-Eval`;
only the explicit human `eval` command writes the local evaluation file. Agent
seat calls and coder-tool writes to the evaluation file are rejected.

## Stats and recommendations

```sh
roster stats
roster stats --ref main..HEAD --evals evals.jsonl
roster recommend --task-class fix --difficulty 4
```

Stats invokes contracts `export-agent-metrics.mjs` and joins the local records,
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

Fleet routing requires **three distinct human evaluations**, not three
automatic failure/pass records. Two evaluations do not qualify for that
tier, though a labeled [starting prior](CAPABILITIES.md) can still select a
registered fleet model. A third distinct human sample for the same class,
model, and effort meets the count threshold, but requested difficulty and
declared context must also be supported. The default routing difficulty is 2.

Example output:

```text
feat: careful-model effort=h accept-rate=66.7% n=3 median-min=20 median-difficulty=4 profile=careful source=evals context_max=32768 concurrency=1 reason=3 distinct human evaluations; declared context is sufficient
```

If no configuration qualifies, output includes `insufficient data` and the
configured fallback as information, not an automatic fleet choice.
`roster recommend` is read-only and shares the registered-profile selector
with `roster run --issue N --runtime builtin --auto-model`. The flag permits
a run-scoped endpoint/model choice without clearing or editing the saved
default. Qualified human data outranks priors; absent both, the run stays a
deterministic stub. An explicit task model outside fleet routing can select the coder as
described in [estimation](ESTIMATION.md). The [next planner task](NEXT.md) also
uses qualifying capacity evidence and carries forward redacted human feedback.
One acceptance can seed an otherwise unconfigured baseline, not a recommendation.
See [routing](ROUTING.md).

## Derived difficulty ceilings and skill notes (FEATURE_SPEC 5.6)

`deriveDifficultyCeilings` reads joined human evaluations without writing a
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
human `at` timestamps; legacy timelines missing timestamps use ledger order.
Explicit `seat` metadata wins, otherwise a session ending in `-planner`,
`-coder`, or `-reviewer` identifies the seat. Older unlabelled runs use the
coder bucket; unknown/stub models do not establish a ceiling.

Each derived reject adds a skill note containing the human comment, redacted
with the existing feedback redactor and blockquoted as **data, not a policy
grant**. Missing comments are stated, not invented. Notes are derived from
the existing local ledger, not written into executable skills or a new DB.
Fleet routing checks the requested seat (coder by default) ceiling in both
evidence and prior tiers; no evaluations for that seat/model preserve prior
routing behavior. Existing median difficulty, context, and sample thresholds
still apply. Ceiling evidence does not pool another seat's performance.

Both `.roster/runs/` and `.roster/evals.jsonl` are ignored by this repository.
When running Roster against another repository, ignore these paths there
before recording seats or evaluations. Keep human free-text feedback local;
the human eval command posts only the compact verdict and reported minutes
when a matching PR exists. These files are evidence, not a task queue.

## Tests

Fixture tests cover automatic and failed seat recording, omitted unknowns,
real zeros, joins and deduplication, human-only evaluations, existing stats
flags, and the three-sample threshold:

```sh
node --test tests/learn.test.mjs tests/eval.test.mjs
```
