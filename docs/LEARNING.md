# Learning from runs and human evaluations

Roster joins the required contracts pack's `AI-Run` export with local run
metadata and human decisions. There is no model API, vault, analytics service,
or separate task database. GitHub Issues and PRs remain the queue.

## Opt in to local run records

Create `.roster/runs` in the repository root before running roster. For example,
in PowerShell:

```powershell
New-Item -ItemType Directory -Force .roster\runs
node src\cli.mjs run --issue 42
```

After successful worktree and assignment setup, bare `run --issue N` appends
one JSONL record to `.roster/runs/runs.jsonl`. Without the directory it records nothing
and does not create `.roster`. Failed setup never records a successful run.
Recording failures are reported explicitly, including the already-created
worktree path; inspect that worktree rather than blindly rerunning setup.

For `run --issue N --runtime builtin`, setup adds no preparation record. It
records one run per completed seat (`roster-N-planner`, then
`roster-N-coder`) with only that seat's reported metrics. If the coder fails,
the planner record may remain. The deterministic stub uses model
`builtin-stub`, with no fabricated tokens.

The bare run command prepares a worker, not a completed commit. Its generated
`session` and `task` are known, but the eventual commit SHA is not: the starting
HEAD is **not** recorded as the result. Published contracts metadata supplies
the SHA later, joined by session and, when available, task.

Only reported fields are written:

| Field | Source |
| --- | --- |
| `sha` | Full commit SHA, when known in a local run record or Git export |
| `session`, `task` | Generated worker assignment; contracts `AI_SESSION`, `AI_TASK` |
| `model` | `AI_MODEL` |
| `effort` | Contracts-normalized `AI_EFFORT`: `l`, `m`, `h`, or `x` |
| `context_used`, `context_max`, `context_out` | Corresponding `AI_CONTEXT_*` environment variables |
| `task_class` | A recognized `feat`, `fix`, `docs`, or `test` prefix in the issue title |

Run metadata normalization uses the contracts parser through
[`resolveContractsPath`](../src/lib/paths.mjs), not a copied parser.
Unknown fields are omitted, not filled with zero. A genuinely reported zero is
preserved; counts beyond JavaScript's safe integer range remain decimal strings.
No environment secrets, issue bodies, prompts, or traces are recorded.

Example with only the assignment known:

```json
{"session":"roster-20260928T120000000Z","task":"issue-42"}
```

Local learning reads all `*.jsonl` files directly inside `.roster/runs`, in
filename order. Records need a full SHA or opaque session identifier; other
fields may be omitted. Matching records are combined rather than counted
twice. Known Git-exported values take precedence over local values.

## Human evaluation

After reviewing the result, a **human** runs:

```sh
roster eval roster-20260928T120000000Z accept 3 n
roster eval 0123456789abcdef0123456789abcdef01234567 rework 4 y
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
unmatched evaluations remain saved but do not influence recommendations.

This appends `.roster/evals.jsonl` at the current Git repository's root, even
when invoked from a subdirectory:

```json
{"session":"roster-20260928T120000000Z","verdict":"accept","difficulty":3,"again":false}
```

Corrections append another record, preserving the history. The last evaluation
for an identical SHA or session wins; SHA matching is case-insensitive. A
SHA-specific evaluation takes precedence over a session-wide evaluation.
Malformed arguments or existing JSONL fail explicitly rather than silently
skipping data.

Optionally, the human can also post the contracts comment using their own
authenticated GitHub CLI:

```sh
gh pr comment 123 --body "AI-Eval: 1|accept|3|n"
```

Roster does not fetch PR comments, post evaluation comments, generate decisions,
or write evaluation trailers. The coder `run` and contracts publishing paths
never write `AI-Eval`; only the explicit human `eval` command writes the local
evaluation file.

## Stats and recommendations

```sh
roster stats
roster stats --ref main..HEAD --evals evals.jsonl
roster recommend --task-class feat
```

Stats invokes contracts `export-agent-metrics.mjs` and joins the local records.
The existing `MODEL`, `EFFORT`, `RUNS`, and `EVALS` columns remain unchanged.
Unknown model or effort displays as `-`.

Without `--ref`, stats includes exported `HEAD` history and local-only runs.
An explicit `--ref` restricts results to exported commits in that revision or
range; local records can enrich matching commits but cannot add out-of-range
or unpublished runs. The existing `--evals PATH` remains supported as an
additional legacy SHA-keyed evaluation file, resolved relative to the caller's
directory. Its SHA evaluations override matching local SHA evaluations. See
[metrics](METRICS.md) for the legacy schema and duplicate validation.

Recommendations group by **task class, model, and effort**. Classification uses
`task_class` when present, otherwise a conventional task prefix such as
`feat-auth`, `fix.cli`, `docs-guide`, or `test-unit`. Issue titles such as
`feat(cli): add a flag` supply the class when roster records the assignment.
Opaque tasks like `issue-42` are not guessed into a class. Unknown models are
not candidates.

For each configuration:

- `n` counts distinct human-evaluated samples of the requested task class.
- Accept-rate is `accept / n`; `reject` and `rework` are not accepts.
- Unevaluated runs do not enter the denominator.
- One session evaluation counts at most once per configuration, even when
  several exported commits share that session.
- At least **3** samples are required. The highest accept-rate wins; ties use
  larger `n`, then model name and effort order (`l`, `m`, `h`, `x`, unknown).
- Difficulty, `again`, and context are retained as evidence, not invented
  quality scores or ranking weights.

Two evaluations still print `insufficient data`, even if a third run exists
without a human evaluation. A third distinct evaluated sample for the same
task class, model, and effort makes that configuration eligible.

Example output:

```text
feat: careful-model effort=h accept-rate=66.7% n=3
```

If no configuration qualifies, the exact output is `insufficient data`.
Recommendations are read-only suggestions; they do not change the worker,
recipe, policy, or publishing configuration.

Both `.roster/runs/` and `.roster/evals.jsonl` are ignored by Git. Keep human
feedback local unless the human explicitly posts the optional PR comment.

## Tests

Fixture tests cover successful and failed runs, omitted unknowns, real zeros,
joins and deduplication, human-only evaluations, existing stats flags, and the
three-sample threshold:

```sh
node --test tests/learn.test.mjs tests/eval.test.mjs
```
