# vLLM OpenAI API and file vault

The client uses Node 20's built-in `fetch` and crypto; there are no runtime
package dependencies. No endpoint is enabled by default, and nothing selects
a hosted service implicitly.

## First profile: vllm-local (DGX Spark)

[`createChat`](../src/llm/openai.mjs) accepts an in-memory configuration object
with an `llm` section. The first endpoint recipe uses the **vLLM OpenAI API
on DGX Spark**. This loopback example assumes Roster runs on the same host:

```js
const config = {
  llm: {
    base_url: 'http://127.0.0.1:8000/v1',
    model: 'your-served-model',
    api_key_optional: true,
  },
};
```

For the builtin YAML config, copy [`roster.config.example.yml`](../roster.config.example.yml),
set `llm.profile: vllm-local` and `llm.model` to the HF handle served by
vLLM, and leave `llm.base_url: ""` for its default (or set a custom vLLM
host URL). The profile sets the URL above and
`api_key_optional: true`; a configured key is still sent. The tracked
example selects no endpoint until you set a profile or custom URL. Hosted
APIs are a later opt-in profile using the same HTTP shape. See
[endpoint profiles](ENDPOINTS.md).
The terminal-only [onboarding wizard](ONBOARDING.md) probes `/models` with
a five-second deadline and asks for a real model ID if discovery fails.
It never probes chat or adds an internet tool.

To check the local server without an API key, replace the model below with
the handle vLLM is serving:

```sh
curl --fail http://127.0.0.1:8000/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"your-org/served-model","messages":[{"role":"user","content":"Say hello."}]}'
```

Loopback works when Roster and vLLM run on the same host. From WSL,
`127.0.0.1` refers to that local environment, not to a separate DGX Spark.
If the server is on another host, use a secure tunnel to its loopback port
or configure a protected, reachable endpoint; do not expose a keyless vLLM
server to an untrusted network.

| Field | Default | Meaning |
| --- | --- | --- |
| `llm.base_url` | empty (disabled) | HTTP(S) API root, including `/v1` when needed |
| `llm.model` | none | Model name; each chat request can override it |
| `llm.api_key_optional` | `false` | Allow requests without a resolved key; set `true` for keyless local servers |
| `llm.api_key_name` | `OPENAI_API_KEY` | Environment variable and vault entry name |
| `llm.request_timeout_ms` | 20 minutes for loopback/private-IP hosts; 120 seconds otherwise | Total HTTP deadline, including response parsing and any HTTP retry delay; a fleet profile can override it for cold-inference gateways |
| `llm.timeout_ms` | same host-based default | Low-level client compatibility alias; `request_timeout_ms` takes precedence |

A streamed call to a remote (non-loopback, non-private-IP) host whose response
body sends no bytes for 180 seconds after the headers arrive is abandoned and
retried once with the same request. Waiting for headers is not watched, so a
cold-inference gateway still gets its full `request_timeout_ms`.
A second silent stream fails as an endpoint stall (`ROSTER_LLM_STALL`), which
seats treat like a timeout rather than a bad TASK. This stops a stuck gateway
from holding a seat until its own stream cutoff. Local hosts are not watched,
because a cold model load can take minutes before the first byte. The
low-level client accepts `stream_idle_timeout_ms` (`0` disables it).

Requests omit `tools` when no tools are offered, because some gateways misroute
an empty `tools` array. A streamed HTTP 200 response that carries a JSON
`error` body, or no data frames at all, is reported as an endpoint error, not
accepted as an empty completion. When the endpoint reports serving a model
unrelated to the requested ID (a gateway alias), the seat prints one
`served-model` warning, and the AI-Run records the served model. If fleet
routing locked the request to a profile, an unrelated served model fails the
run instead; Roster never silently substitutes a backend for a locked route.

An absent, empty or whitespace-only `base_url` makes `createChat` return
`null`, without accessing the vault or network. Keep the caller's existing
stub in that case. `roster prepare` uses
[`runIssue`](../src/lib/issue.mjs) without an LLM call or config loader.
The existing builtin coder loop calls this
factory; it does not add a separate runner, planner, or task queue.

The builtin [planner](../src/planner/stub.mjs) and
[coder](../src/runtime/loop.mjs) use this client through a shared adapter
when `.roster/config.yml` sets the vLLM URL in `llm.base_url` or selects
`llm.profile` for vLLM, Ollama, LM Studio, or OpenAI. The resolved
`llm.api_key_env` is both the environment-variable and vault entry name:
a non-empty environment value wins without reading the vault, while an
unset or empty value falls back to the vault. Keys are optional for the
`vllm-local` profile; a resolved key is still sent when present. The
empty-URL stub makes no chat request.
Both seats pass only reported token usage to their AI-Run metadata. The
client does not log API keys or HTTP request/response bodies.

An enabled hook is used as follows:

```js
import { createChat } from './src/llm/openai.mjs';

const chat = createChat(config);
if (chat !== null) {
  const { message, usage } = await chat({
    messages: [{ role: 'user', content: 'Explain this task.' }],
  });
  // Pass message and usage to the caller; do not log request/response bodies.
}
```

The result is `{ message, usage }` with an optional `finish_reason` of `stop`
or `tool_calls`. `message` is the first choice's message object, including
tool calls when present, and `usage` is the provider's usage object or
`null` if omitted. The low-level client rejects unsupported finish reasons with
an error that names only the reason, never the response body. Its `lastResponse`
retains the real response model and reported usage even on that error.
Extra request fields, such as `temperature` and `max_tokens`, pass through.
A request may set `stream: true`; the client then adds
`stream_options: { include_usage: true }`, assembles the server-sent chunks
into the same `{ message, usage }` result and reads `usage` from the final
chunk. Each content delta is forwarded as a redacted `delta` event as it
arrives, while tool-call deltas only accumulate: the assembled message carries
complete tool-call arguments, so no caller can act on a partial argument. A
stream that fails before it completes raises the stream error rather than an
empty response, and a server that answers a streaming request with a JSON body
is parsed as a plain completion. The builtin adapter takes `stream: true` for
the plan, draft, test and review seats; the research and side-question helpers
stay non-streaming. After every call that reports usage, streaming or not, the
client emits
one `usage` event with the server's prompt and completion tokens, plus the
cached prompt tokens when the endpoint reports them. Token counts are never
estimated from message length, and the event reaches the rail and `/usage`
only, never the run log or a trailer. Streamed text reaches the live transcript
only; it is never written to the run log or the debug log.

The shared builtin adapter accepts `stop` and `tool_calls` and grants one
`length` retry per request, at most three per chat instance (the coder keeps
one instance for its run).
For a non-docs-slice run it logs `Response truncated. Retrying.`, discards the
truncated completion without executing or replaying its tool calls, and retries
at the same completion cap, asking the model to split large output into
`edit_file` hunks or several smaller write/edit calls. Halving the cap cannot
help because the overflowing content must still be emitted. If the truncated
response has no visible content and no tool call, reasoning spent the whole
cap: the retry doubles the cap, up to 32768 (a larger configured cap is kept),
and asks the model to think briefly and answer directly.
A docs slice starts with reasoning disabled (`reasoning_effort: none`, and
local DeepSeek `thinking: false`) before its first request and never sends a
completion cap below 8192, including the continue turn.
On `length` it logs `Response truncated. Continuing the same message.`, keeps
the partial assistant message, and sends exactly one *continue* turn at the
same cap; a truncated tool call is still dropped rather than replayed, and the
continue turn then asks for the complete call. This applies regardless of the
docs filename, preserves completed writes, and has no second retry for that request.
Builtin requests without an explicit `max_tokens` start at a cap scaled to the
reasoning effort, because thinking shares the completion cap with the answer:
4096 with reasoning off, 8192 for low, 16384 for medium/high and 32768 for
xhigh/max, never above a quarter of the profile `context_max` nor below 4096.
A seat may supply its own cap, such as
the 8192 floor for docs slices. The cap and, for docs slices, disabled reasoning remain in effect for later
requests; it is an output cap, not the model context capacity.
The retry is an extra coder model turn, independent of failed-test repairs.
A second `length` after a saved README-only Status edit is treated as a
late summary success when the exact `## Status` heading and one body line are
already present. A README-only docs slice is offered `read_file`, `write_file`
and `run_test` only: `list_dir` and `search_text` are not in its tool list, and
a requested directory walk fails the seat instead of starting another turn.
Otherwise a second `length`, or any other unsupported reason,
is terminal and appears by name in failure evidence and review. Completed
README writes are preserved; the invalid response cannot overwrite them.
Response-backed metrics retain
the last real model/usage, while total coder usage includes the retry.

The client POSTs JSON to `{base_url}/chat/completions`. It sends
`Authorization: Bearer ...` **only** when a non-empty key resolves, even when
keys are optional. URL credentials, query strings, fragments and redirects
are rejected to avoid accidental credential forwarding. A missing required
key fails before any HTTP call.

The low-level client retries at most once for HTTP 429, and at most once for
a transient gateway failure (HTTP 502/503/504, or an HTTP 200 JSON error body
in place of a stream). `Retry-After` seconds or dates
are honored within the same deadline; missing or invalid headers use a
one-second delay. A repeated transient gateway failure is a route failure:
an auto-routed seat continues on another eligible fleet profile with its
worktree edits intact instead of ending the run. Other HTTP errors, invalid
responses, network errors and
timeouts fail explicitly. Errors contain fixed descriptions or HTTP status
numbers, never upstream bodies, URLs, keys or underlying error causes. The
client does not log requests, responses or credentials.

The slice planner additionally retries the same request once after a timeout
when its configured deadline is shorter than the 20-minute cold-start allowance.
The timed-out request is aborted before that retry; no partial tool calls execute.
The human transcript and metadata logs explicitly report the retry. Cancellation,
authentication failures, and network errors do not trigger this recovery. A second
timeout, or a timeout after the full cold-start allowance, stops the run before
coder, tests, reviewer, or publication. This is bounded recovery, not an
automatic endpoint/model switch or an unlimited warmup loop. For a public gateway
backed by cold local inference, set its fleet `request_timeout_ms: 1200000` rather
than relying on the public-host default.

## Later hosted profile: same HTTP shape, vault key

Store a key through stdin, then use configuration such as:

```js
const config = {
  llm: {
    base_url: 'https://api.openai.com/v1',
    model: 'your-hosted-model',
    api_key_name: 'OPENAI_API_KEY',
    api_key_optional: false,
  },
};
```

[`resolveSecret`](../src/lib/secrets.mjs) checks the named environment
variable first. A non-empty value wins without opening the vault. An unset
or empty variable falls back to the same vault name; a missing entry returns
`undefined`. Vault failures are errors, not missing-key fallbacks.
For the builtin YAML config, use `llm.api_key_env` as that name. GitHub App
credential names and PEM private keys are refused by the LLM vault; keep the
App key outside the checkout and this vault.

## File vault

The default location is `~/.roster/vault`, or
`%USERPROFILE%\.roster\vault` on Windows. No secrets are written into the
checkout. Vault locations inside Git worktrees (including through symlinked
parents) are rejected, as are symlinked vault directories and entry files.

```sh
roster vault set OPENAI_API_KEY
roster vault list
roster vault get OPENAI_API_KEY
```

`set` requires piped/redirected stdin and reads until EOF; it never accepts a
secret argument or echoes the value. For example, pipe the output of your
existing secret manager into `roster vault set OPENAI_API_KEY`.
Do not type literal credentials into shell history. One trailing LF or CRLF
is removed; other whitespace is preserved. Empty values are rejected.
`list` prints sorted names only, one per line. `get` emits the exact value
only when stdout is redirected or piped; it refuses an interactive terminal
and errors on a missing name. The interactive shell's `/vault get NAME`
reports presence without revealing the value.

The [`createFileVault`](../src/vault/file.mjs) API exposes async `set(name,
value)`, `get(name)` and `list()` methods. Names are case-sensitive, 1-64
ASCII letters/digits/underscores and start with a letter or underscore.
Missing entries return `undefined`; listing a missing vault returns `[]`.
Reads do not create an absent vault. A `directory` option is available for
isolated tests, but cannot point into a Git worktree.

Values use AES-256-GCM with a fresh 12-byte nonce per write, a 16-byte
authentication tag and the name as authenticated data. Each encrypted entry
is replaced atomically. A random 32-byte key is created once in `.key` using
an exclusive atomic publication. Entry filenames encode names, not values;
names are metadata, not confidential. Corrupt entries or a missing/invalid
key fail rather than silently resetting storage.

On POSIX, vault directories use `0700` and files use `0600`, including
temporary files. Existing permissions are tightened on access. On Windows,
where Node's modes are not access controls, the vault uses the current
user's owner-only inheritable ACL via built-in PowerShell and `icacls`.
Permission-setting failures are fatal; they do not permit an insecure write.

The encryption key is local, not password-derived or protected by a
hardware keystore. Encryption prevents storing API values as plaintext; it
does **not** protect against someone who can read both the key and entries,
including a compromised user account or an administrator. Keep backups of
the entire vault private and outside Git. Losing `.key` loses access to
existing values. Never commit keys, tokens, PEMs or environment files.

## Tests

```sh
node --test tests/openai.test.mjs tests/vault.test.mjs tests/secrets.test.mjs tests/cli.test.mjs tests/issue.test.mjs
```

Tests mock HTTP and isolate vaults under `os.tmpdir()`; no live provider or
real home-directory secret is required.
