# LLM endpoints

Start with the vLLM OpenAI API on DGX Spark. The
[schema 1 config example](../roster.config.example.yml) supplies defaults for
the builtin planner and coder. Copy it to ignored `.roster/config.yml` to
configure a model-backed `roster run --issue N`.
An empty endpoint keeps the deterministic stub; `roster prepare --issue N`
is the manual handoff path. See [SDLC](SDLC.md) for execution and publishing.

## Schema 1

- `schema: 1` identifies the config shape.
- `llm.base_url`, `llm.model`, and `llm.profile` are deliberately empty
  strings. Choose a profile and model, or set a custom base URL and model.
  With an empty model, only an explicit `run --auto-model` may choose from
  human-evaluated runs; without it, a configured endpoint fails before
  creating a worktree. The example does not select a provider or contact
  an endpoint.
- `profiles.vllm-local` is the first named profile, with URL
  `http://127.0.0.1:8000/v1` and `api_key_optional: true`.
  `profiles.ollama`, `profiles.lmstudio`, and `profiles.openai` are
  alternatives. Selecting `llm.profile` uses that endpoint and key name;
  `llm.base_url` must then remain empty. An explicit base URL requires an
  empty profile. Older configs without `vllm-local` can still select its
  built-in default.
- `llm.api_key_env: ROSTER_API_KEY` names an environment variable, not a key.
  Both seats use the [OpenAI-compatible client](LLM.md): a non-empty environment
  value wins over the same-named vault entry, and authorization is sent only
  for a resolved key. Leave both unset for a keyless local server.
- `llm.effort: m` records medium effort and `llm.context_max: 0` means an unknown
  context limit. These are run metadata, not provider-specific request options.
- `planner.turn_budget` bounds the planner's JSON responses; it has no file
  tools. `seat.id: coder` and `seat.principal: coder` select the coder and
  contracts role. `seat.turn_budget` bounds the coder loop and `seat.tools`
  lists its allowed tools. The human-owned policy still governs publishing.
- `paths.memory` names the `.roster/memory/coder.jsonl` memory file.
  `paths.skills`, `paths.asks`, and `paths.worktrees` name repository-relative
  directories (`skills`, `.roster/asks`, and `.worktrees`). GitHub issues and PRs
  remain the work queue.

Keep every field in the example when overriding configuration. Older private
configs without `planner` or `profiles` remain valid; the planner gets one
turn and the named profiles retain their documented defaults. The underlying
[chat factory and vault](LLM.md) accept an in-memory API with
`llm.api_key_optional` and `llm.api_key_name`; these are not top-level builtin
YAML fields. Use `profiles.vllm-local.api_key_optional` in the YAML example.

Use the OpenAI-compatible `/v1` base URL, not a full `/chat/completions` URL.
With the server running, `GET <base_url>/models` lists model IDs; use an
available ID for `llm.model`. Keep local servers bound to loopback unless
remote access is deliberately secured.

In the interactive [Roster shell](REPL.md), `/model MODEL` and `/effort h`
persist those two fields to ignored `.roster/config.yml` without editing the
tracked example. `/model clear` leaves the model empty for opt-in
`/run N --auto-model`. Select the endpoint profile in that private config.
See [model routing](ROUTING.md) for the three-evaluation threshold.

## Local profiles

### vllm-local (DGX Spark)

Run vLLM on DGX Spark. When Roster runs on the same host, set
`llm.profile: vllm-local` and keep `llm.base_url: ""` in the copied config,
then set `llm.model` to the served HF handle returned by `/v1/models`.
The profile permits a keyless local server and sends a key when one resolves.
If server authentication is required, configure `ROSTER_API_KEY` or set
`profiles.vllm-local.api_key_optional: false`. See the [curl and networking
guide](LLM.md) when Roster runs under WSL or away from the DGX Spark.

### Ollama

Set `llm.profile: ollama`, leave `llm.base_url: ""`, start Ollama and pull a
model, then set `llm.model` to that installed model's ID. The profile uses
`http://127.0.0.1:11434/v1` and its OpenAI-compatible
`/v1` API, not the native `/api` endpoints. For an unauthenticated local server,
leave the API-key environment variable unset.

### llama.cpp

Start `llama-server` with a GGUF model and a loopback listener. For port `8080`,
set `llm.base_url` to `http://127.0.0.1:8080/v1`; adjust the port if configured
differently. Set `llm.model` to an ID returned by `/v1/models`. Leave the API-key
environment variable unset only when server authentication is disabled.

### LM Studio

Load a model and start the local OpenAI-compatible server in LM Studio. For
port `1234`, Set `llm.profile: lmstudio`; the default URL is
`http://127.0.0.1:1234/v1`. Override the profile URL if LM Studio shows a
different port. Set `llm.model` to an ID returned by
`/v1/models`. Leave the API-key environment variable unset only when server
authentication is disabled.

## Later profile: OpenAI

Hosted inference is an explicit opt-in, not the default. Requests leave the
local machine and may incur charges. Keep the other schema 1 fields and
replace the `llm` section with:

```yaml
llm:
  base_url: ""
  model: your-hosted-model
  api_key_env: OPENAI_API_KEY
  effort: m
  context_max: 0
  profile: openai
```

Choose a model available to your OpenAI account and replace the example
`llm.model`. Supply `OPENAI_API_KEY` through your environment or file vault;
never put
its value in YAML, endpoint URLs, documentation, or Git. Do not commit tokens,
private keys, or `.env` files.
