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

## Fleet CLI

```sh
roster fleet list
roster fleet add --id sample --base-url https://gpu.example.invalid/v1 --model fictional/coder --context 32768 --concurrency 2 --hardware fictional-gpu --task-class feat,fix
roster fleet probe sample
roster fleet probe sample --set-model fictional/new-coder
roster fleet default sample
roster fleet remove sample
```

Replace fictional endpoints and models with actual reachable values.
`add` GETs `/models`, prints the served IDs, and requires a model in that
inventory. In a TTY, omitting `--model` asks for a selection (first served
ID by default); omitting `--context` asks for a positive token limit.
Non-TTY add requires `--id`, `--base-url`, `--model`, and `--context`, so
there is no prompt or capacity guess. Concurrency defaults 1, hardware
defaults `unspecified`, and task-class hints are optional. Duplicate IDs
and invalid inputs are refused without overwriting registered profiles.

`probe ID` prints the current inventory but leaves the saved model and
files unchanged. Only explicit `--set-model MODEL` (or a TTY selection
with bare `--set-model`) updates the catalog. If that endpoint/model is
the current default, its private config is updated in the same paired
write. Recheck the declared context limit when changing models; discovery
returns IDs, not a capacity benchmark.

`default ID` explicitly updates the project's configured endpoint/model,
provider and context limit without modifying permission choices. `remove`
refuses the last profile and the currently configured endpoint; choose
another default first. Missing IDs are errors, never a model fallback.
Read-only list/probe operations do not silently alter the default config.
No command invokes an App publisher, adds a queue, or launches a worker.
First-run routing uses explicitly labeled [capability priors](CAPABILITIES.md),
not scraped scores. Qualifying local human evaluations take precedence;
a prior cannot nominate a model that is absent from the private catalog.
