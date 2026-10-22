# Copilot feasibility: offline decision packet

Evidence checked for #570, under #569, and ported to native Rust in
[`rust/capabilities.rs`](../rust/capabilities.rs). This is an offline
verification and decision packet, not a live provider or Eve acceptance.

Spec trace: [FEATURE_SPEC](FEATURE_SPEC.md) sections **5.2** (explicit model and
staffing), **5.4** (bounded scoped tools), **5.5** (independent review and gates),
and **5.8** (deadline, cleanup, credential separation). Unknown usage also
honors section **5.6**. Sections **3** and **7** preserve default deny, local-first
operation, one writer per worktree, GitHub as the queue, and no invented metrics.

## Recommendation and gates

| Candidate | Decision for this slice | Reason |
| --- | --- | --- |
| Offline synthetic mapping | GO for independent review only | Existing Roster shapes and scoped read tools can be exercised without a provider, dependency or outbound request. |
| External CLI as a builtin single-model-turn transport | NO-GO on current evidence | A non-interactive prompt is an agent run, not necessarily one inference call. Public tool filters do not establish a custom Roster callback bridge or physical credential containment. |
| Node SDK as a builtin single-model-turn transport | NO-GO under current constraints | Adds SDK/native runtime dependencies and wraps the CLI-owned automatic agent loop; does not establish a supported one-call stop/handoff contract. |
| Distinct optional full-agent worker | Selected design direction (#571) | Viable with separate provenance, scoped Roster callbacks, OS credential isolation, bounded costs and an independent reviewer ([`COPILOT_WORKER_DESIGN.md`](COPILOT_WORKER_DESIGN.md)). Never represented as a builtin Roster seat. |
| Live smoke, onboarding, entitlement or calibration | Gated (#572, #573) | Requires explicit data/account/model/policy/spend authorization and offline preflight/scorecard validation. |

The official [agent-loop guide][loop] states that the SDK is a transport while
the CLI executes tools and automatically requests subsequent model turns. One
`send` can therefore consume multiple API calls before `session.idle`. Listening
to events and calling `abort()` after observing a turn is **not** proof that the
next inference has not already started.

## Official interface comparison

Only official public GitHub documentation and its public SDK reference/metadata
were retrieved. No private repository data was submitted to a provider.
SDK references are pinned to public upstream commit
`6be623b01b194a61466385e3f708639ec98ce8ac`; its package metadata specifies CLI
runtime `1.0.96-2`.

| Topic | External official CLI | Official SDK | Consequence for Roster |
| --- | --- | --- | --- |
| Installation/runtime | npm `@github/copilot` requires Node **22+**. WinGet on Windows; Homebrew/install script on macOS/Linux; executable downloads. Windows prerequisite PowerShell **6+**. [Install][install] | `@github/copilot-sdk`; documented Node **^20.19.0 or >=22.12.0**. Managed stdio/TCP runtime, existing URI connection, or experimental in-process FFI. [Node reference][node] | Roster's native Rust runtime keeps zero Copilot SDK dependencies; any external executable requires explicit platform and containment qualification. |
| Dependencies/platforms | Separately installed platform executable. | SDK metadata lists `koffi` 3.2.1, `vscode-jsonrpc` 8.2.1 and `zod` 4.3.6 plus optional `@github/copilot-sdk-<platform>` packages. [Metadata][package] | No SDK or npm dependency is added to Roster. |
| Process/session | `-p` executes a prompt non-interactively then exits. CLI supports interactive sessions and resume/continue modes. [Programmatic][programmatic] [Commands][commands] | `createSession`, `resumeSession`, `send`, event subscriptions, `sendAndWait`; sessions and state directories are separate from Roster's TASK/RESULT/REVIEW. [Node reference][node] | Undefined, missing idle, process errors or timeout cannot be converted into a successful seat result. |
| Tool control | `--available-tools`, `--excluded-tools`, `--allow-tool`, `--deny-tool`, URL/path permission options. [Programmatic][programmatic] | `defineTool` callbacks or raw JSON schemas; available/excluded tool filters, source-qualified custom/builtin/MCP selection. [Changing tools][changing] | An adapter must select only Roster callbacks and validate arguments/scope again. Empty custom tools are not proof of no auto-tools. |
| Model discovery | `--model` selects a model; interactive model selection is not an entitlement check. [Programmatic][programmatic] | `listModels()` and `models.list` supply model capabilities/pricing information. [Usage][usage] | No auto fallback. Requested model is not proof of the model that served a call. |
| Cancellation/deadlines | Interactive Esc/Ctrl+C cancellation and shutdown shortcuts are documented. [Commands][commands] | `abort()` cancels currently processing message; `disconnect()` frees local resources; `stop()` returns cleanup errors. [Node reference][node] | A wait timeout is not proof of server cancellation, process exit, stopped callbacks or deletion of persisted data. |
| Budgets | A prompt may include multiple inference calls and tool steps. | `sessionLimits.maxAiCredits` is a **soft cap**, checked after calls; one response can exceed it. [Session limits][limits] | No universal concurrency/rate ceiling or guaranteed maximum bill inferred. Human spend policy must account for overshoot. |
| Per-call metrics | Text/stats or JSONL output is available, without a verified per-call token/cost capture contract in this spike. [Programmatic][programmatic] | `assistant.usage`: reported `model`, `inputTokens`, `outputTokens`, `cost`; ephemeral, not replayed on resume. [Usage][usage] [Events][events] | Missing values stay unknown, not zero or configured-model fallback. Cost field is provider-specific, not automatically USD. |

## Authentication, telemetry and physical containment

The [CLI authentication guide][cli-auth] documents:

- Precedence: `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, stored OAuth
  keychain token, then `gh auth token` fallback.
- OAuth storage: Windows Credential Manager, macOS Keychain, Linux libsecret.
  `/logout` removes local stored authentication but does not revoke the token
  on GitHub.
- `COPILOT_OFFLINE=true` disables GitHub auth attempts and telemetry, but BYOK
  can still send prompts to a remote endpoint.

The documented [organization server-to-server route][server-auth] needs
Copilot Requests repository permission **Read & write**, organization
enablement and the current **All repositories** installation scope. Installation
tokens expire after **one hour** and must be supplied through the runtime
`COPILOT_GITHUB_TOKEN` environment path. This is a separate human/org-authorized
inference App design, not authorization to repurpose or change Roster's
publishing App.

## Native Rust offline verification (`rust/capabilities.rs`)

[`offline_copilot_inference_context`](../rust/capabilities.rs) and
[`run_offline_copilot_turn`](../rust/capabilities.rs) implement the bounded
single-turn feasibility contract in native Rust:

- Input is a bounded Roster-shaped text request (`system`/`user` messages only),
  an exact `read_file` tool definition, explicit file allowlist, a 64 KiB
  payload bound, 2–4 transcript events and at most one scoped tool call.
- `assistant.usage`, optional `fixture.tool_request`, `assistant.message`, and
  `session.idle` are validated in strict order. Malformed/unknown requests,
  duplicate/late usage, missing idle, empty completion, and scope escapes fail
  closed before or during scoped read verification.
- Missing reported model is `null`, not the requested model. Missing usage is
  `null`; partial counts stay missing and reported zero remains zero.
  Provenance is always `offline-fixture` with `publishable: false`.
- Parent environment input is discarded into an empty inference descriptor
  (`env: {}`, `useLoggedInUser: false`, `executable: null`,
  `ghFallback: "unavailable-no-process"`).

## Official sources

[install]: https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli
[cli-auth]: https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli
[sdk-auth]: https://docs.github.com/en/copilot/how-tos/copilot-sdk/auth/authenticate
[server-auth]: https://docs.github.com/en/copilot/how-tos/copilot-sdk/auth/server-to-server-tokens
[node]: https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/nodejs/README.md
[package]: https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/nodejs/package.json
[platforms]: https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/nodejs/src/runtimeArtifacts.ts
[loop]: https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/docs/features/agent-loop.md
[changing]: https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/docs/features/changing-tools.md
[limits]: https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/docs/features/session-limits.md
[usage]: https://docs.github.com/en/copilot/how-tos/copilot-sdk/features/usage-and-billing
[events]: https://docs.github.com/en/copilot/how-tos/copilot-sdk/features/streaming-events
[commands]: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference
[programmatic]: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-programmatic-reference
[config]: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference
[telemetry]: https://docs.github.com/en/copilot/how-tos/copilot-sdk/observability/opentelemetry
[billing]: https://docs.github.com/en/copilot/concepts/billing-and-usage/individuals/billing
[rate]: https://docs.github.com/en/copilot/concepts/billing-and-usage/individuals/usage-limits
