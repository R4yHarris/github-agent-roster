# Explicit LLM routing

Roster uses the strict [schema 1 config](../roster.config.example.yml).
Its `llm.profile` is empty by default, as are `llm.base_url` and
`llm.model`: the run is a deterministic stub. Copy the example to ignored
`.roster/config.yml` before changing settings. The named profiles supply
an endpoint and an API-key **name**, never a key value:

| Profile | Default endpoint | Key name |
| --- | --- | --- |
| `vllm-local` | `http://127.0.0.1:8000/v1` | `ROSTER_API_KEY` (optional) |
| `ollama` | `http://127.0.0.1:11434/v1` | `ROSTER_API_KEY` |
| `lmstudio` | `http://127.0.0.1:1234/v1` | `ROSTER_API_KEY` |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` |

Select `llm.profile` and a model, or leave the profile empty and supply a
custom `llm.base_url` and model. Do not set a profile and a custom base URL
together. Hosted inference is never selected implicitly. The configured
environment key wins over a same-named file-vault entry; no App PEM belongs
in the LLM vault. See [endpoint setup](ENDPOINTS.md).

In the [human shell](REPL.md), `/model MODEL` and `/effort h` atomically
persist those fields to the private config. `/model clear` removes the
chosen model without selecting another one. An empty model with a
configured endpoint is an explicit error on a normal run.

`--auto-model` is the **only** opt-in path that can fill an empty model:

```sh
roster run --issue N --runtime builtin --auto-model
```

The shell equivalent is `/run N --auto-model`. The issue title supplies a
recognized `feat`, `fix`, `docs`, or `test` task class. Roster joins
contracts AI-Run history with local runs and explicit human evaluations;
[`recommend`](../src/lib/learn.mjs) considers distinct evaluated samples
for the same class, model, and effort. Unknown models and `builtin-stub`
are not candidates. At least **three** evaluated samples must support a
configuration. `roster recommend --task-class feat` remains read-only.

When a qualifying model exists and an endpoint is configured, Roster
uses that model and recommended effort for **both seats in the current
run**, including their AI-Run metadata. It does not persist the automatic
choice to `.roster/config.yml`. With fewer than three samples, no
recognized task class, or no endpoint, the run stays on the stub: no
model request, code edit, or test execution is claimed. A manually chosen
model cannot be overridden by `--auto-model`; clear it first. Metrics
errors remain errors, not silent routing fallbacks. GitHub Issues and PRs
remain the [board](BOARD.md), not a model-routing database. See
[human evaluations](LEARNING.md) for the evidence threshold.
