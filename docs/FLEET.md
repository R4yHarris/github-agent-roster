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

After selecting a real model, onboarding uses its positive context limit
reported by `/v1/models`, or asks for `Model context tokens` when unavailable,
then `Add more endpoints later with roster fleet add. Continue? [yes]`.
SGLang reports `max_model_len`; a value of `1048576` is saved as
`context_max: 1048576` without another question. Other supported context
fields are listed in [onboarding](ONBOARDING.md#probing-v1models).
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
ID by default); omitting `--context` uses the selected model's reported
positive token limit or asks when missing. Explicit `--context` still overrides
discovery. Non-TTY add requires `--id`, `--base-url`, and `--model`; it also
requires `--context` if discovery does not report a valid capacity.
There is no capacity guess. Concurrency defaults 1, hardware
defaults `unspecified`, and task-class hints are optional. Duplicate IDs
and invalid inputs are refused without overwriting registered profiles.

`probe ID` prints the current inventory and known context limits but leaves the saved model and
files unchanged. Only explicit `--set-model MODEL` (or a TTY selection
with bare `--set-model`) updates the catalog. If that endpoint/model is
the current default, its private config is updated in the same paired
write. A reported context limit updates the selected model's catalog capacity
and the active default config together. If none is reported, the existing
declared limit remains and the command reminds you to recheck it. These server
declarations are not capacity benchmarks.

`default ID` explicitly updates the project's configured endpoint/model,
provider and context limit without modifying permission choices. `remove`
refuses the last profile and the currently configured endpoint; choose
another default first. Missing IDs are errors, never a model fallback.
Read-only list/probe operations do not silently alter the default config.
No command invokes an App publisher, adds a queue, or launches a worker.
First-run routing uses explicitly labeled [capability priors](CAPABILITIES.md),
not scraped scores. Qualifying local human evaluations take precedence;
a prior cannot nominate a model that is absent from the private catalog.

## Opt-in routing

`roster recommend --task-class fix --difficulty 3` and
`roster run --issue N --auto-model` use the same
[route selector](ROUTING.md). At least three distinct human evaluations
with sufficient rated difficulty and declared context outrank starting
priors; otherwise matching prior/class hints guide the first run.
Output states the profile, model, and `evals` versus `prior` reason.
Concurrency is a weak tie-break only, never a parallel-execution command.

`--auto-model` is the explicit permission to select a different endpoint
and model for one run. It does not require clearing a saved default and
does not rewrite that default. Ordinary runs and recommendations do not
switch or persist a fleet choice. Models outside this catalog cannot be
automatically nominated, even if they appear in evaluation history.

## Assisted setup

After `roster onboard` has saved a working default endpoint/model, run
`roster fleet assist` in a terminal. The onboarded default model asks the
[interview questions](../templates/fleet/ASK.md) one endpoint at a time:
ID, base URL, actual served model ID, hardware, context, concurrency,
task-class hints, and notes. The [discovery guide](../templates/fleet/DISCOVER.md)
covers `/v1/models`, port 8000, WSL/Windows reachability, and authorized LAN
or DGX endpoints without private addresses in tracked files.

Each proposed profile is shown as YAML. The harness appends to private
`.roster/fleet.yml` **only after the user explicitly says yes** at
`Write this profile? [no]`, using the existing fleet validator. No/blank
does not write; `done` or `/quit` stops the interview. Model suggestions
cannot invent endpoints or model IDs, overwrite existing profiles, or
grant policy. If the model endpoint fails, local template questions remain
available without the model.

The [two-profile template](../templates/fleet/FLEET.example.yml) contains
fictional examples with comments for every field. It is not automatically
loaded as a fleet or treated as evidence of measured model capacity.

Assistance requires the **project's** onboarded private config with a real
vLLM base URL and model. It does not take `AI_MODEL` or an installation
default as a substitute. Piped input or redirected output prints
`roster fleet assist needs a terminal` and exits 2 before loading settings
or contacting a server. It uses the configured OpenAI-compatible chat
client and selected key name, with no model-invokable tools.

The harness validates each answer, GETs `/models` only for an operator-
supplied safe URL, and accepts an inventory ID (or explicitly supplied
actual model when that probe failed). A reported positive context limit becomes
a validated probe fact and skips the context question; otherwise the operator
supplies it. Other limits, hardware, hints, and notes come from the operator,
not guesses in a model response. A final model
proposal must exactly match those recorded facts after normalization.
An invented value, malformed JSON, or tool request is reported and
switches to the local template interview; its rejected values are never
shown as a trusted preview or written.

`Write this profile? [no]` is always a separate user question, never a model
decision. Blank/no skips the profile; yes appends the validated row without
changing the saved default config. Duplicate IDs and concurrent catalog
changes are refused rather than overwriting another profile. `done`,
`/quit`, or Ctrl+C stops without writing an unconfirmed entry; already
confirmed entries remain. The known credential values are neither sent as
interview facts nor stored in the catalog. This setup has no publication,
merge, network-scan, or internet-search tool.

Run `node --test tests/fleet-assist.test.mjs tests/fleet-cli.test.mjs` for
model/probe mocks, yes-only writes, invalid URLs, invented proposals,
template fallback, cancellation, and stale-catalog regressions.
