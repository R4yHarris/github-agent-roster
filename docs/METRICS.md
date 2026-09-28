# Local metrics

`src/lib/metrics.mjs` reads compact AI-Run JSONL by invoking the sibling
`github-agent-contracts/scripts/export-agent-metrics.mjs` with Node. Set
`GITHUB_AGENT_CONTRACTS` to that clone's path, or use the default sibling
clone when running the CLI. It runs against local Git history (`cwd`,
defaulting to the current directory) and does not contact GitHub or any
analytics service.

```sh
node src/cli.mjs stats --ref HEAD --evals evals.jsonl
```

`roster stats` accepts `--ref REVISION_OR_RANGE` (default `HEAD`) and optional
`--evals PATH`. It does not fetch evaluations implicitly. Code can import
`loadMetrics({ contractsPath, cwd, ref, evalsPath })`,
`summarizeMetrics(records)`, and `formatMetrics(groups)`; the module does not
parse CLI flags.

The optional local evaluation file is UTF-8 JSONL, one record per full
40- or 64-hex-digit commit SHA:

```json
{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","verdict":"accept","difficulty":3,"again":false}
```

`verdict` is a nonempty string, `difficulty` is a finite nonnegative number,
and `again` is a boolean. `loadMetrics` returns exported run records with an
`evaluation` object or `null` added to each. SHA matching is case-insensitive;
evaluations outside the selected Git history are not included. Duplicate SHAs,
malformed JSONL or fields, unreadable evaluation files, and exporter failures
raise errors with source/line or command context. An absent `--evals` option
leaves all `evaluation` fields `null`.

`summarizeMetrics` counts runs and matched evaluations by `model` and `effort`
across versions and providers; `formatMetrics` produces a table with `MODEL`,
`EFFORT`, `RUNS`, and `EVALS` columns. Effort uses the exporter's `l`, `m`, `h`,
`x`, or `-` for unknown, sorted in that order per model. Empty history prints
`No AI-Run records found.` No rankings, model selection, quality scores, or
machine learning are calculated; verdict, difficulty, and again remain
available on joined records for later use.

Run the focused tests with `node --test tests/metrics.test.mjs`.
