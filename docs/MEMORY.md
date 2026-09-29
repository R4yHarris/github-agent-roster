# Seat memory is a notebook

The coder appends compact JSONL records to `.roster/memory/coder.jsonl` in the
roster installation. This directory is [gitignored](../.gitignore). A configured
`paths.memory` is still honored; the planner has its own sibling
`planner.jsonl`. Neither seat reads the other's notebook.

Memory is **not the team's board**. GitHub Issues and PRs remain the queue and
the source of task state. A memory entry does not claim an issue, grant
authority, or change TASK.md scope.

Each new coder entry contains:

| Field | Meaning |
| --- | --- |
| `time` | UTC ISO timestamp |
| `issue` | GitHub issue number, or `null` for a local/demo task |
| `task` | The task's AI-Run identifier |
| `changed` | Count and a short list of successfully written task paths |
| `tests` | Last observed `node --test` exit code, or `Not run.` |
| `next_gap` | First failure, the stub's missing endpoint, or no reported gap |

`session`, `status`, and a short factual `summary` remain available for existing
consumers. The record comes from observed tool results, not the model's claimed
success. Stub runs record no code changes and no executed tests.

[`appendMemory`](../src/runtime/memory.mjs) uses append-only file access: it
never rewrites previous bytes. An incomplete final line, unsafe path, symlink,
non-file target, or invalid record fails explicitly. Entries contain compact
scalar metadata and are capped at 4096 bytes. File/message body fields and
credential fields are rejected. Summaries are single-line and bounded, with
an explicit omission marker. Known environment credentials and common token
forms are redacted on both append and read. Model responses, test output,
and full source-file bodies are not copied into coder records.

`readMemory({ file, repoRoot, limit: n })` is the tail operation: it reads
backward in bounded chunks and returns at most `n` validated JSONL lines,
oldest to newest. A missing notebook or `limit: 0` returns an empty list;
negative and non-integer limits fail. Old compact records remain readable
without migration or rewriting. The [context pack](CONTEXT.md) requests the
latest 20 records and may omit older ones to fit its budget.

The notebook is not a secret store. Keep credentials in the operator-managed
vault outside Git, and do not rely on redaction to make arbitrary sensitive
prose safe to persist.
