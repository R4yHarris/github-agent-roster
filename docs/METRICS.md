# Local metrics

[`buildRun`](../src/metrics/run.mjs) constructs two compact schema 1 AI-Run
lines in a builtin issue run: one for `roster-N-planner` from planner token
reports and one for `roster-N-coder` from coder token reports. Both use task
`issue-N`. Unknown version, context, or counts stay unset (`-` in
the compact line); a missing usage report never becomes an estimate. The
empty-URL stub emits no AI-Run trailer and records no model or LLM usage.
The publisher receives the coder's known `AI_*` fields
only, not the model API key: the contracts SDK supports one AI-Run trailer per
published code commit. Both completed seat runs are printed and recorded
separately in `.roster/runs`. Contracts owns the trailer format.

An `unknown` model is a bug, not a default. Both builtin publication and
standalone `/publish` pass the configured `llm.model` as `AI_MODEL`, falling
back to `ROSTER_MODEL` when config has no model. Builtin seats use that same
selection for their LLM requests. Publication clears inherited run metadata
and never forwards the LLM API key. A completed coder's metadata wins over
later configuration changes. With no model, all `AI_*` run fields are omitted,
so the SDK adds no AI-Run. The SDK's separate required `AI-Model` trailer still
has its legacy fallback when no model is supplied; Roster does not invent a
model to replace it.

The pinned contracts `v0.2.0` schema does **not** accept `AI_PROVIDER=vllm`.
Roster encodes vLLM as its supported `local` provider (`openai` for the explicit
OpenAI profile), without modifying the submodule. `AI_MODEL_VERSION` is the
known environment value or `-`; `AI_EFFORT` comes from config. Builtin sessions
are `roster-N-planner` and `roster-N-coder`, with `AI_TASK=issue-N`.
The printed manual publication instructions include the coder's environment
fields; set those before running the SDK command directly.

`src/lib/metrics.mjs` reads compact AI-Run JSONL by invoking contracts
`scripts/export-agent-metrics.mjs` with Node. Contracts resolution checks the
required `v0.2.0` submodule first, then `GITHUB_AGENT_CONTRACTS`, then the sibling
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
evaluations do not create runs. An explicit `--evals` file's entries override
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

`summarizeMetrics` counts runs and matched evaluations by `model` and `effort`
across versions and providers; `formatMetrics` produces a table with `MODEL`,
`EFFORT`, `RUNS`, and `EVALS` columns. Effort uses the exporter's `l`, `m`, `h`,
`x`, or `-` for unknown, sorted in that order per model. An unknown local model
also displays as `-`. Empty history with no local runs prints
`No AI-Run records found.` Stats does not rank or select models. The separate
`roster recommend --task-class feat|fix|docs|test` command suggests the best
accept-rate only with at least three human-evaluated samples per configuration;
it does not change the worker or invent quality scores.

Run the focused tests with
`node --test tests/metrics.test.mjs tests/learn.test.mjs tests/eval.test.mjs`.
