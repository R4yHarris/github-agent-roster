# Local metrics

[`createChat`](../src/llm/openai.mjs) retains an immutable `lastResponse`
snapshot containing the response's `model` (or the actual request model when
the response omits it) and reported `usage.prompt_tokens` /
`usage.completion_tokens`. It retains neither prompts nor response text,
API keys, or arbitrary usage fields. Each seat uses its last successful
completion, not accumulated usage from earlier requests. A final response
without usage clears the reported token slots rather than reusing an earlier
response's counts. Aggregate `result.usage` remains available separately for
operational totals.

[`buildRun`](../src/metrics/run.mjs) constructs a canonical `run.metrics`
object and a contracts-compatible compact schema 1 AI-Run line. Builtin issue
runs record planner, coder, and reviewer sessions (`roster-N-planner`,
`roster-N-coder`, and `roster-N-reviewer`) with task `issue-N`. The same known
model, provider, selected effort, prompt/completion counts, context capacity, session,
and task are written into `.roster/runs/*.jsonl`. Legacy `context_used` and
`context_out` fields mirror reported prompt and completion tokens for existing
stats consumers. Context capacity comes only from the selected fleet profile
or `llm.context_max`; an absent or zero capacity stays unknown. Missing counts
are omitted, not estimated as zero or 1,000,000. An explicitly reported zero
is retained. Unknown slots are `-` in the compact line.

Opt-in [best-of-N attempts](MULTIAGENT.md#opt-in-best-of-n-slice-attempts) retain
each candidate's measured coder model/usage in an eval-ready JSONL record with a
bounded `attempt` object: batch ID, count/index, declared redacted profile/hardware,
tests/excellence/red-green/shadow gate outcomes, review verdict, changed lines,
monotonic duration, and selected winner (`0` means none). Human `roster eval`
targets the distinct candidate session. Losing gate outcomes never become
invented human rejection evaluations. Only the winner's actual coder AI-Run is
published; losing evidence persists after its worktree is removed.

The separate live `.roster/runs/<session>.log` is append-only operational
activity; plain-language actions (including chosen coder effort) are printed
to stderr separately. For an issue, one
`roster-N-coder.log` contains all three seats; timestamps, mode, elapsed time,
HTTP status/error classes, and tool/file names are metadata only. It stores no
prompts, responses, file bodies, or credentials. Offline status shows its last
seat and complete line. JSONL metrics/stats ignore `.log` files; a log is not
an AI-Run token report or a human evaluation.

Builtin assignments record machine-local `started` provenance as soon as the
worktree is prepared, before routing and planning. Seat records and the terminal
`completed`, `failure`, or `cancellation` share one run ID. A returned planning
failure is not completion; a planning-only pause records a session with a paused
outcome. Failures before a repository assignment exists cannot produce a
repository-scoped record. An unsuccessful best-effort write is explicitly
reported as incomplete durable history; opt-out is intentional, not an error.
`completed` means execution ended, not human acceptance; a deterministic stub
retains an `unverified` outcome.

Session provenance records retain the requested model from the actual seat
start and the canonical served model and token counts from its response-backed
run object. Observed seat start/end times, selected route/profile/hardware, and
measured duration accompany them. Missing counts stay `unknown`, reported zero
stays zero, and inherited `AI_*` counts cannot fill absent evidence. Records
contain no endpoint credentials or prompt/source snapshots.

The same typed session record retains observed `evidence.verification` for
coder tests: numeric `exit_code`, explicit `skipped` status, the original
`full_suite_exit_code` when available, and existing baseline/transient
classification metadata. Baseline files are classified outside the slice;
this field alone does not claim a successful baseline rerun. File lists retain
at most eight entries of 128 characters each; the baseline count retains the
full observed count. No stdout or stderr is retained. Documentation-only
verification records a skip without inventing a test exit; a stub cannot
claim verified tests.

Reviewer `evidence.review` retains the observed verdict, `completed` and
`queried` flags, at most sixteen unmet check IDs, and a bounded artifact
reference (`REVIEW.md` plus the SHA-256 of its actual written content).
The reference is an integrity identifier, not an archived review body.
Failed and incomplete reviews retain their real status; missing evidence
remains absent. These records survive worktree removal and use the existing
typed/redacted machine store. They do not change gates or create human evals.
This implements [FEATURE_SPEC sections 5.5, 5.6 and 5.8](FEATURE_SPEC.md).

Every recorded builtin seat occurrence has a distinct durable session ID
within its run. The first retains the existing session ID; later occurrences
use `<session>-attempt-2`, `<session>-attempt-3`, and so on.
`evidence.attempt.sessionId` links to the original operational session and
`evidence.attempt.index` gives its one-based occurrence. Query by run ID to
retrieve the complete retry/review-repair history; an exact original session
query still identifies the first occurrence. Each occurrence keeps its own
observed model, timing and verification/review evidence. Earlier failure is
never overwritten by a later pass. Operational metrics, AI-Run and publication
session IDs are unchanged, as are store duplicate protection and opt-out.
This implements [FEATURE_SPEC sections 5.4, 5.5, 5.6 and 5.8](FEATURE_SPEC.md).

Durable provenance identity hashes the canonical origin rather than the checkout
path, so moves, reclones, and linked worktrees share a history identity.
HTTPS and SSH spellings normalize together; credentials do not affect identity.
Non-default ports, forks, and case-sensitive paths on arbitrary Git servers stay
distinct (GitHub paths are case-insensitive). Local origins resolve relative to
the checkout. Repository/worktree state containment and explicit state-path
overrides remain separate and unchanged. This implements
[FEATURE_SPEC sections 5.6 and 5.8](FEATURE_SPEC.md).

Legacy path-derived hashes are not automatically reassigned or deleted. Existing
records remain available through explicit history-store diagnostics; identity
mismatch still fails closed. Preserve state before an explicit reinitialization.
This slice does not implement the broader offline/no-origin fallback described
in [state ownership](STATE.md#6-repository-identity).

Effort records the orchestrator-selected compact tier, not a hardcoded
medium or a model's private reasoning. Local DeepSeek-V4.1 medium is
normalized to high. `none` is encoded as `-` in the existing contracts schema
(which has no disabled-thinking tier) and omitted from the JSONL effort field;
the request itself explicitly disables thinking. `reasoning_content` is
discarded before model messages are returned or replayed and is never written
to seat memory.

The publisher derives `AI_PROVIDER`, `AI_MODEL`, `AI_EFFORT`,
`AI_CONTEXT_USED`, `AI_CONTEXT_MAX`, `AI_CONTEXT_OUT`, `AI_SESSION`, and
`AI_TASK` from the completed coder's metrics object, not process metadata
or later configuration changes. The SDK receives the actual response model
with `--model`; it supports one AI-Run trailer per published code commit.
Inherited token counts, capacity, model version, and LLM API keys cannot
override completed seat evidence. After a confirmed App merge, the issue
comment includes the measured model, provider, known prompt/completion counts
and context capacity, session, task, and compact AI-Run. The issue stays open
for human AI-Eval.

An `unknown` model is a bug, not a default. Missing or invalid publication
model IDs fail before the SDK; its required `AI-Model` trailer has no unknown
fallback. The empty-URL stub emits no AI-Run and cannot publish code.

### GHCP-only attribution

Hardcoded Copilot session settings apply only to GHCP-authored commits with
no completed Roster seat run in this process. [`buildGhcpRun`](../src/metrics/run.mjs)
requires `AI_MODEL` or an explicit publication `--model`; it never substitutes
the configured served model or `ROSTER_MODEL`. It always sets
`AI_PROVIDER=github-copilot` and version `-`, even if a vLLM or cloud profile
is configured for a later run. Effort comes only from `AI_EFFORT` (including
`max` -> `x`); unknown effort stays `-`. This Max session declares
`AI_MODEL=GPT-6.1-Sol` and `AI_EFFORT=x`.
An explicitly declared positive `AI_CONTEXT_MAX` can describe that session's
capacity only when there is no completed run object. It is never a measured
used-token count. Inherited `AI_CONTEXT_USED` and `AI_CONTEXT_OUT` are cleared.
Thus GHCP input/output are always `-`: `-/1000000` when that capacity is
declared, otherwise `-/-`; output stays `-`. Wrappers generate a `ghcp-<pid>`
session unless a GHCP session is explicitly declared, and use the issue ID,
explicit `AI_TASK`, or current branch slug as task.
REPL `/publish ... --model MODEL` can declare the Copilot model without changing
the served model. A completed seat object takes precedence over even that flag.
Manual handoffs print every metadata variable, including empty values that
clear inherited fields. See both blocks in [`.env.example`](../.env.example).

| Field | GHCP (no completed seat) | vLLM seat | Cloud OpenAI-profile seat |
| --- | --- | --- | --- |
| Provider in AI-Run | `github-copilot` | `local` (`vllm` in journal) | `openai` |
| Model | Explicit `--model` or `AI_MODEL` | Last response model, otherwise request model | Last response model, otherwise API request model |
| Version | `-` | `-` when unknown | `-` when unknown |
| Effort | `AI_EFFORT`; `x` for declared Max | Seat configuration | Seat configuration |
| Input / output | `-` / `-`, never inherited counts | Last `usage.prompt_tokens` / `usage.completion_tokens` | Last `usage.prompt_tokens` / `usage.completion_tokens` |
| Maximum context | Declared `AI_CONTEXT_MAX`, otherwise `-` | Fleet/config `context_max`, otherwise `-` | Fleet/config `context_max`, otherwise `-` |
| Session | `ghcp-<date-or-pid>` | `roster-<issue>-<seat>` | `roster-<issue>-<seat>` |
| Task | Branch slug or issue ID | Issue ID | Issue ID |

Missing seat usage is omitted, not replaced with the GHCP path. Publication
uses the same compact schema for all three sources; identity remains the App.

The pinned contracts schema does **not** accept `AI_PROVIDER=vllm`.
Roster encodes vLLM as its supported `local` provider (`openai` for the explicit
OpenAI profile), without modifying the submodule. The local run journal
retains the actual `provider: "vllm"` for the named vLLM profile or explicit
`llm.provider`, while the compact AI-Run and published trailer use `local`.
For live seats, an explicit `llm.provider` takes precedence over the selected
profile; an unprofiled compatible endpoint defaults to `local`, never
inherited `AI_PROVIDER=github-copilot`. Onboarding sets `llm.provider` to
`vllm`. Seat model version stays unknown (`-`) unless independently known;
the client does not infer one from Copilot's environment. Effort comes from
the seat configuration.
The printed manual publication instructions include every AI-Run environment
field. Empty values clear inherited unknown slots; apply those as well as the
known coder values before running the SDK command directly.

`src/lib/metrics.mjs` reads compact AI-Run JSONL by invoking contracts
`scripts/export-agent-metrics.mjs` with Node. Contracts resolution checks the
required contracts submodule first, then `GITHUB_AGENT_CONTRACTS`, then the sibling
clone; see [the dependency guide](DEPENDENCY.md). It runs against local Git
history and joins `.roster/runs/*.jsonl` and `.roster/evals.jsonl` when present.
It does not contact GitHub or any analytics service.

```sh
node src/cli.mjs stats --ref HEAD --evals evals.jsonl
```

`roster stats` accepts `--ref REVISION_OR_RANGE` and optional `--evals PATH`.
Without `--ref`, it reads `HEAD` and includes local-only runs. With an explicit
`--ref`, it includes only exported commits in the selected history, enriched by
matching local records. It never fetches evaluations from GitHub. Code can import
`loadMetrics({ contractsPath, cwd, ref, evalsPath })`,
`summarizeMetrics(records)`, and `formatMetrics(groups)`; the module does not
parse CLI flags. The CLI resolves the current repository root so local learning
works from subdirectories; library callers supply that root as `cwd` (default:
the current directory).

The additional legacy `--evals PATH` file is UTF-8 JSONL, one record per full
40- or 64-hex-digit commit SHA. Relative paths use the caller's directory:

```json
{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","verdict":"accept","difficulty":3,"again":false}
```

`verdict` is a nonempty string, `difficulty` is a finite nonnegative number,
and `again` is a boolean. This legacy schema and its duplicate-SHA rejection
remain supported. `loadMetrics` returns joined run records with an `evaluation`
object or `null` added to each. SHA matching is case-insensitive; unmatched
legacy evaluations without model/class do not create runs. Self-contained local
human evaluations can supply learning evidence without an exported run. An explicit `--evals` file's entries override
local SHA evaluations.

The default `.roster/evals.jsonl` uses the stricter human `roster eval` schema:
at least one SHA or session (both when known), `accept|reject|rework`, integer
difficulty 1-5, and boolean `again`, plus known model/class, optional actual
minutes, local comment, and an ISO timestamp. Legacy records still load.
It is append-only, so the last decision for a target wins. A SHA
evaluation takes precedence over a session evaluation. Local run metadata joins
by full SHA or by session and compatible task; known exported fields win.
Duplicate local run records do not add extra counts. See
[learning](LEARNING.md) for recording and correction semantics.

Malformed JSONL or fields, unreadable files, and exporter failures raise errors
with source/line or command context. Missing default learning files are optional;
a missing explicit `--evals` path is an error.

`summarizeMetrics` groups by model, task class, and effort across versions and
providers. `formatMetrics` keeps `MODEL`, `EFFORT`, `RUNS`, `EVALS` and adds
`TASK_CLASS`, unique sample `N`, `ACCEPT` rate, `MEDIAN_MIN`, and
`MEDIAN_DIFFICULTY`. Unknown values display as `-`; known zero minutes stay zero.
Effort uses the exporter's `l`, `m`, `h`, `x`, or `-`. Empty history with no local
evidence prints `No AI-Run records found.` Stats does not select models.

`roster recommend --task-class fix --difficulty 4` ranks accept-rate only among
configurations with at least three distinct samples and median difficulty at
least four. Recorded excellence failures are rejects, not accepts, even when
tests passed. No qualifying candidate prints insufficient data and config's
model/effort default. See [learning](LEARNING.md) for exact denominators and
missing-data handling; there is no hidden score.

Run the focused tests with
`node --test tests/openai.test.mjs tests/run-metrics.test.mjs tests/builtin.*.test.mjs tests/issue-board.test.mjs tests/metrics.test.mjs tests/learn.test.mjs tests/eval.test.mjs`.

## Delivery evidence

`roster stats --delivery` (or `/stats --delivery`) groups evidence by actual
response model, declared fleet hardware, and seat. Add `--json` for stable,
diffable JSON. CLI history/evaluation selectors still work:

```sh
roster stats --delivery --json --ref HEAD --evals evals.jsonl
```

The pure `deliveryMetrics(records)` and `formatDeliveryMetrics(groups)` helpers
live in `src/lib/delivery-metrics.mjs`. `loadMetrics({ delivery: true })` preserves
individual local seat attempts when enriching published commits; ordinary stats
retains its existing commit/model/effort semantics. Explicit `--ref` still excludes
unpublished local attempts. Neither stats nor the aggregation contacts an endpoint
or GitHub, starts seats, or changes staffing decisions.
Published SHA links enrich evaluations, but a final commit's model or usage never
overwrites the response-backed metadata of an earlier attempt.

New local journal rows include `seat` and a bounded `delivery` snapshot. Its
opaque `id` identifies one invocation's pipeline; `attempt` is the seat attempt
within it. Repeated snapshots of an attempt count once (the latest journal row
wins). Rerunning an issue is a new pipeline, not a fabricated continuation of
an older timeline. Hardware is the selected profile's declaration at execution
time, redacted before storage, never inferred from model names or today's fleet.
Standalone and historical runs without that binding report `unknown`.
An explicit `/review --again` on an in-memory completed run retains that pipeline's
ID and increments the reviewer attempt. It cannot turn a repaired review into a
new first-pass success; new gate observations do not reuse the earlier attempt's
counts. A resumed legacy run without pipeline evidence still has unknown history.

| Output | Source and calculation |
| --- | --- |
| `leadMinutes` | Median `(delivery.pr_merged_at - delivery.ask_created_at) / 60000`, once per pipeline. The existing App-authenticated merge/comment check reads the PR's `merged_at` and issue's `created_at`; verified timestamps are cached on the completed seat snapshots. No commit date or local publish-end time substitutes for a GitHub merge timestamp. |
| `coderMinutes` | Median summed `delivery.duration_ms / 60000` for coder attempts in each pipeline/dimension group. Timing uses the existing monotonic seat clock and includes in-seat tools/tests, but not planner, reviewer, publication, or time waiting between invocations. |
| `firstPassYield` | Completed reviewer pipelines passing on reviewer attempt 1 / pipelines with a completed review. `delivery.review_verdict` comes from the harness review result, not prose. Missing/incomplete reviews are not passes; passing only on a later attempt is not first-pass. This metric belongs to the reviewer seat/model, not an inferred coder verdict. |
| `rework` | Sum of observed `delivery.review_repairs` plus unique linked human evaluations whose latest `evaluation.verdict` is `rework`. Review repairs are counted once when the repair starts producing journal evidence; endpoint retries and scope expansions are not review repairs. Human evaluation correction semantics remain unchanged. |
| `gateFailures` | Observed failures by `red-green`, `shadow`, `self-review`, and `verifying-reviewer`, from `delivery.gates[gate].failures`. Live gate events increment `checks`/`failures`; a later green result does not erase an earlier failure. Skipped, unavailable, exempt, or unexecuted gates contribute no observation, not a passing zero. |
| `estimateErrorMinutes` | Median signed `evaluation.minutes - delivery.estimate_min`, once per linked evaluation in a group. Estimate is the pre-coder TASK estimate; actual minutes must be the human's recorded value, not the seat timer. |

`samples` lists the observed denominators; `gateChecks` lists observed checks.
Counts describe available evidence, not a complete history when coverage is
partial. Missing metric values are the literal string `unknown` in both table
and JSON; a measured zero remains `0`. Missing usage is still omitted from the
underlying AI-Run; delivery timing never manufactures token counts. Old journals
are not retroactively assigned hardware, timestamps, repair counts, or verdicts.
Local-only/manual publications without verified cached GitHub timestamps keep
lead time unknown. A cache-write failure is reported even if the PR already merged;
it must not trigger a second publication.

This implements FEATURE_SPEC sections 5.6 (per-seat/model feedback), 5.7 (stats),
and 5.8 (hardware evidence), respecting section 7's prohibition on fake metrics.
