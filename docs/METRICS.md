# Local metrics

[`buildRun`](../src/metrics/run.mjs) constructs two compact schema 1 AI-Run
lines in a builtin issue run: one for `roster-N-planner` from planner token
reports and one for `roster-N-coder` from coder token reports. Both use task
`issue-N`. Unknown provider, version, context, or counts stay unset (`-` in
the compact line); a missing usage report never becomes an estimate. The
empty-URL stub emits both seat records labeled `builtin-stub`, with no
invented LLM usage. The publisher receives the coder's known `AI_*` fields
only, not the model API key: the contracts SDK supports one AI-Run trailer per
published code commit. Both seat runs are printed and, when local learning
is enabled, recorded separately. Contracts owns the trailer format.

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
SHA **or** session, `accept|reject|rework`, integer difficulty 1-5, and boolean
`again`. It is append-only, so the last decision for a target wins. A SHA
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
