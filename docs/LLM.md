# Local-first chat and file vault

The client uses Node 20's built-in `fetch` and crypto; there are no runtime
package dependencies. No endpoint is enabled by default, and nothing selects
a hosted service implicitly.

## Configuration and hook

[`createChat`](../src/llm/openai.mjs) accepts an in-memory configuration object
with an `llm` section. For a local OpenAI-compatible server:

```js
const config = {
  llm: {
    base_url: 'http://127.0.0.1:11434/v1',
    model: 'your-local-model',
    api_key_optional: true,
  },
};
```

| Field | Default | Meaning |
| --- | --- | --- |
| `llm.base_url` | empty (disabled) | HTTP(S) API root, including `/v1` when needed |
| `llm.model` | none | Model name; each chat request can override it |
| `llm.api_key_optional` | `false` | Allow requests without a resolved key; set `true` for keyless local servers |
| `llm.api_key_name` | `OPENAI_API_KEY` | Environment variable and vault entry name |
| `llm.timeout_ms` | `30000` | Total HTTP deadline, including response parsing and any retry delay |

An absent, empty or whitespace-only `base_url` makes `createChat` return
`null`, without accessing the vault or network. Keep the caller's existing
stub in that case. The prepare-only [`runIssue`](../src/lib/issue.mjs) path has
no LLM call or config loader. The existing builtin coder loop calls this
factory; it does not add a separate runner, planner, or task queue.

The builtin [planner](../src/planner/stub.mjs) and
[coder](../src/runtime/loop.mjs) use this client through a shared adapter
when `.roster/config.yml` sets a custom `llm.base_url` or selects
`llm.profile` for Ollama, LM Studio, or OpenAI. The resolved
`llm.api_key_env` is both the environment-variable and vault entry name:
a non-empty environment value wins without reading the vault, while an
unset or empty value falls back to the vault. Keys are optional for local
endpoints such as Ollama at `http://127.0.0.1:11434/v1`; a resolved key
is still sent when present. The empty-URL stub makes no chat request.
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
`null` if omitted. Truncated or unsupported finish reasons are errors.
Extra non-streaming request fields, such as `temperature` and `max_tokens`,
pass through. Streaming is not supported.

The client POSTs JSON to `{base_url}/chat/completions`. It sends
`Authorization: Bearer ...` **only** when a non-empty key resolves, even when
keys are optional. URL credentials, query strings, fragments and redirects
are rejected to avoid accidental credential forwarding. A missing required
key fails before any HTTP call.

There is at most one retry, only for HTTP 429. `Retry-After` seconds or dates
are honored within the same deadline; missing or invalid headers use a
one-second delay. Other HTTP errors, invalid responses, network errors and
timeouts fail explicitly. Errors contain fixed descriptions or HTTP status
numbers, never upstream bodies, URLs, keys or underlying error causes. The
client does not log requests, responses or credentials.

## Hosted API: same client, vault key

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
node src/cli.mjs vault set OPENAI_API_KEY
node src/cli.mjs vault list
node src/cli.mjs vault get OPENAI_API_KEY
```

`set` requires piped/redirected stdin and reads until EOF; it never accepts a
secret argument or echoes the value. For example, pipe the output of your
existing secret manager into `node src/cli.mjs vault set OPENAI_API_KEY`.
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
