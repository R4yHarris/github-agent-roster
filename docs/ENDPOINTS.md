# LLM endpoints

Start with a local OpenAI-compatible server. The
[schema 1 config example](../roster.config.example.yml) is documentation for
configuration; the current CLI does not load it or make LLM requests. These
profiles do not add runtime behavior.

## Schema 1

- `schema: 1` identifies the config shape.
- `llm.base_url` and `llm.model` are deliberately empty strings. Choose both
  explicitly; the example does not select a provider or model.
- `llm.api_key_env: ROSTER_API_KEY` names an environment variable, not a key.
  `llm.api_key_optional: true` describes a server that permits requests without
  authentication. If a server requires authentication, set it to `false` and
  supply the key through that environment variable.
- Top-level `effort: m` means medium effort. It is not a guarantee that a server
  supports a provider-specific reasoning parameter.
- Top-level `seat: coder` selects the coder seat, not GitHub permissions. The
  human-owned policy and contracts pack still govern publishing.
- `paths.memory`, `paths.skills`, and `paths.asks` name repository-relative
  local directories (`./memory`, `./skills`, and `./asks`). `paths.worktrees`
  names `./.worktrees`, matching the existing one-task loop. The example does
  not create directories or change the CLI's paths. GitHub issues and PRs
  remain the work queue.

Use the OpenAI-compatible `/v1` base URL, not a full `/chat/completions` URL.
With the server running, `GET <base_url>/models` lists model IDs; use an
available ID for `llm.model`. Keep local servers bound to loopback unless
remote access is deliberately secured.

## Local profiles

### Ollama

Set `llm.base_url` to `http://127.0.0.1:11434/v1`. Start Ollama and pull a model,
then set `llm.model` to that installed model's ID. Use its OpenAI-compatible
`/v1` API, not the native `/api` endpoints. For an unauthenticated local server,
leave `llm.api_key_optional: true`.

### llama.cpp

Start `llama-server` with a GGUF model and a loopback listener. For port `8080`,
set `llm.base_url` to `http://127.0.0.1:8080/v1`; adjust the port if configured
differently. Set `llm.model` to an ID returned by `/v1/models`. Leave
`llm.api_key_optional: true` only when server authentication is disabled.

### LM Studio

Load a model and start the local OpenAI-compatible server in LM Studio. For
port `1234`, set `llm.base_url` to `http://127.0.0.1:1234/v1`; use the port
shown by LM Studio if different. Set `llm.model` to an ID returned by
`/v1/models`. Leave `llm.api_key_optional: true` only when server authentication
is disabled.

## Later profile: OpenAI

Hosted inference is a later, explicit opt-in, not the default. Requests leave
the local machine and may incur charges. Keep the other schema 1 fields and
replace the `llm` section with:

```yaml
llm:
  base_url: "https://api.openai.com/v1"
  model: ""
  api_key_env: ROSTER_API_KEY
  api_key_optional: false
```

Choose a model available to your OpenAI account and fill in `llm.model`.
Supply `ROSTER_API_KEY` through your environment or secret manager; never put
its value in YAML, endpoint URLs, documentation, or Git. Do not commit tokens,
private keys, or `.env` files.
