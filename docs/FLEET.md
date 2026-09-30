# Endpoint fleet catalog

Onboarding still selects one default endpoint. The fleet is a private catalog
of other available model endpoints, not a queue, worker swarm, or concurrency
runtime. GitHub Issues and PRs remain the board and forge.

The schema is a `profiles` list in ignored `.roster/fleet.yml`:

```yaml
profiles:
  - id: example-coder
    base_url: https://gpu.example.invalid/v1
    model: fictional/coder
    provider: vllm
    context_max: 32768
    concurrency: 2
    hardware: fictional-gpu
    task_class: [feat, fix, docs, test]
    notes: "Describe measured limits in your private catalog."
```

Use actual reachable endpoints and served model IDs in your private file,
never credentials in URLs or notes. The tracked [example](../examples/fleet.yml)
uses fictional names and is **not** automatically loaded as a live fleet.
An absent private catalog means no fleet is configured.

[`fleet.mjs`](../src/lib/fleet.mjs) validates unique opaque IDs, an actual model,
HTTP(S) base URLs without credentials/query/fragment, positive safe-integer
`context_max`, and `concurrency >= 1`. A URL's trailing slash is removed;
`/v1` is appended if missing. Full `/models` or `/chat/completions` request
URLs are refused. `task_class` is optional and, when supplied, contains
distinct `feat`, `fix`, `docs`, or `test` hints. Hardware and notes are
single-line metadata, not benchmark evidence or permission grants.

The small YAML subset supports one root, two-space list items, four-space
fields, double-quoted strings, decimal integers, and inline task-class lists.
Unknown/duplicate fields, duplicate IDs, malformed values, symlinks,
non-file catalog paths, and files over 64 KiB fail explicitly. Load from the
current worktree root, including when invoked from a nested directory.
Use `profiles: []` for an explicitly empty catalog.

No endpoint is contacted by parsing or loading the catalog. Test the schema
with `node --test tests/fleet.test.mjs`. Catalog registration does not change
App identity, human-owned policy, publication rights, or the default model.

## Onboarding default

After selecting a real model, onboarding asks for the model's context limit
in **tokens**, then `Add more endpoints later with roster fleet add. Continue? [yes]`.
This is separate from the context-pack character budget. Confirmed setup
saves the chosen endpoint/model to both private config and the first fleet
profile, `id: default`, with concurrency 1. Other profiles are retained.

Only this seeded default may use `context_max: 0`, explicitly meaning
unknown capacity; other catalog profiles require a positive context limit.
No capacity is guessed from a model name. Config and fleet are prepared
before replacement and saved together with rollback on write failure.
Declining Continue or final confirmation writes neither file.
