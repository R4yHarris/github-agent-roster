# LLM endpoints

Start with a local OpenAI-compatible server. The
[schema 1 config example](../roster.config.example.yml) supplies defaults for
the builtin planner and coder. Copy it to ignored `.roster/config.yml` to
configure the opt-in `run --issue N --runtime builtin` path.
An empty endpoint keeps the deterministic stub; bare `run --issue N` remains
prepare-only. See [SDLC](SDLC.md) for execution and publishing.

## Schema 1

- `schema: 1` identifies the config shape.
- `llm.base_url` and `llm.model` are deliberately empty strings. Choose both
  explicitly; the example does not select a provider or model.
- `llm.api_key_env: ROSTER_API_KEY` names an environment variable, not a key.
  The configured coder uses the [OpenAI-compatible client](LLM.md), which
  sends authorization only for a resolved key. Leave the variable unset for
  a keyless local server; the planner still needs it for a keyed endpoint.
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
configs without `planner` remain valid with a one-turn planner. The separate
[chat factory and vault](LLM.md) accept an in-memory API with
`llm.api_key_optional` and `llm.api_key_name`; those are not builtin YAML fields.

Use the OpenAI-compatible `/v1` base URL, not a full `/chat/completions` URL.
With the server running, `GET <base_url>/models` lists model IDs; use an
available ID for `llm.model`. Keep local servers bound to loopback unless
remote access is deliberately secured.

## Local profiles

### Ollama

Set `llm.base_url` to `http://127.0.0.1:11434/v1`. Start Ollama and pull a model,
then set `llm.model` to that installed model's ID. Use its OpenAI-compatible
`/v1` API, not the native `/api` endpoints. For an unauthenticated local server,
leave the API-key environment variable unset.

### llama.cpp

Start `llama-server` with a GGUF model and a loopback listener. For port `8080`,
set `llm.base_url` to `http://127.0.0.1:8080/v1`; adjust the port if configured
differently. Set `llm.model` to an ID returned by `/v1/models`. Leave the API-key
environment variable unset only when server authentication is disabled.

### LM Studio

Load a model and start the local OpenAI-compatible server in LM Studio. For
port `1234`, set `llm.base_url` to `http://127.0.0.1:1234/v1`; use the port
shown by LM Studio if different. Set `llm.model` to an ID returned by
`/v1/models`. Leave the API-key environment variable unset only when server
authentication is disabled.

## Later profile: OpenAI

Hosted inference is a later, explicit opt-in, not the default. Requests leave
the local machine and may incur charges. Keep the other schema 1 fields and
replace the `llm` section with:

```yaml
llm:
  base_url: "https://api.openai.com/v1"
  model: ""
  api_key_env: ROSTER_API_KEY
  effort: m
  context_max: 0
```

Choose a model available to your OpenAI account and fill in `llm.model`.
Supply `ROSTER_API_KEY` through your environment or secret manager; never put
its value in YAML, endpoint URLs, documentation, or Git. Do not commit tokens,
private keys, or `.env` files.
