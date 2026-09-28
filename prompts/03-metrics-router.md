# Prompt 03 — metrics ingest (no model yet)

Add src/lib/metrics.mjs that reads JSONL from
`node $GITHUB_AGENT_CONTRACTS/scripts/export-agent-metrics.mjs`
and joins optional AI-Eval lines from a local evals.jsonl:

`{"sha":"...","verdict":"accept","difficulty":3,"again":false}`

`roster stats` prints counts by model and effort. No ML. No default model picker yet.
Tests with fixture JSONL.
