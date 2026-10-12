# Copilot feasibility: offline decision packet

Evidence checked **2026-10-12** for #570, under #569. This is a test-only spike,
not a live provider or Eve acceptance. Parent technical review is recorded
below; human architecture/auth/dependency decisions remain required.

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
| Node SDK as a builtin single-model-turn transport | NO-GO under current constraints | Adds SDK/native runtime dependencies and narrows supported Node versions. SDK send/idle wraps the CLI-owned automatic agent loop; the fixture does not establish a supported one-call stop/handoff contract. |
| Distinct optional full-agent worker | Human architecture decision, not authorized implementation | May be viable with separate provenance, scoped Roster callbacks, OS credential isolation, bounded costs and an independent reviewer. Must not be represented as a builtin Roster seat without proving equivalent controls. |
| Live smoke, onboarding, entitlement or calibration | BLOCKED | Needs explicit data/account/model/policy/spend authorization and the subsequent slices. |

The official [agent-loop guide][loop] says the SDK is a transport, while the CLI
executes tools and automatically requests subsequent model turns. One `send`
can therefore consume multiple API calls before `session.idle`. Listening to
events and calling `abort()` after observing a turn is **not** proof that the
next inference has not already started. Tool filtering, terminal custom tools
or pending-tool APIs are promising research surfaces, not a verified replacement
for Roster's own single-turn loop here. No unsupported `maxTurns: 1`, auth
disable flag, hard spend ceiling or guaranteed sandbox behavior is assumed.

Before #571, Ray must choose:

1. Keep the existing zero-dependency/Node >=20 policy, or explicitly approve
   pinned SDK/native-runtime dependencies and the narrower Node support range.
   An externally installed CLI avoids an npm dependency in Roster, but is still
   an additional executable/runtime operational dependency.
2. Require a proven Roster-controlled one-inference-call/tool-result handoff,
   or approve distinct full-agent semantics with separate attempt/usage metrics.
3. Select a human-authorized authentication route and physical isolation
   design. No publisher App credential may enter inference.
4. Approve repository/task data transfer, account/license and organization
   feature/model policies, and a finite additional-usage budget. Public model
   examples/discovery are not evidence of Ray's eligibility.
5. Authorize a low-risk live smoke separately, after offline review and
   containment/dependency decisions. Further calibration needs another gate.

## Official interface comparison

Only official public GitHub documentation and its public SDK reference/metadata
were retrieved. No private repository data was submitted to a provider.
SDK references are pinned to public upstream commit
`6be623b01b194a61466385e3f708639ec98ce8ac`; its package metadata specifies CLI
runtime `1.0.96-2`. This is an inspected source snapshot, not an installed
release or a compatibility certification.

| Topic | External official CLI | Official Node SDK | Consequence for Roster |
| --- | --- | --- | --- |
| Installation/runtime | npm `@github/copilot` requires Node **22+**. WinGet on Windows; Homebrew/install script on macOS/Linux; executable downloads. Windows prerequisite PowerShell **6+**. [Install][install] | `@github/copilot-sdk`; documented Node **^20.19.0 or >=22.12.0**. Managed stdio/TCP runtime, existing URI connection, or experimental in-process FFI. [Node reference][node] | Node >=20 cannot be silently replaced with either npm CLI's or SDK's requirement. Native executable installation has its own platform validation gate. Nothing installed here. |
| Dependencies/platforms | Separately installed platform executable, not a Roster npm dependency. | SDK metadata lists `koffi` 3.2.1, `vscode-jsonrpc` 8.2.1 and `zod` 4.3.6. Optional `@github/copilot-sdk-<platform>` packages bundle runtime wrapper and native `runtime.node`; README describes eight platform packages. [Metadata][package] [Node reference][node] | Not zero runtime dependencies. Public platform inventory lists macOS arm64/x64, Linux glibc arm64/x64, Linux musl arm64/x64, Windows arm64/x64. [Platform metadata][platforms] No size estimate or cross-platform test fabricated. |
| Process/session | `-p` executes a prompt non-interactively then exits. CLI supports interactive sessions and resume/continue modes. [Programmatic][programmatic] [Commands][commands] | `createSession`, `resumeSession`, `send`, event subscriptions, `sendAndWait`; sessions and state directories are separate from Roster's TASK/RESULT/REVIEW. `sendAndWait` waits for idle and can return **undefined** if no message arrived. [Node reference][node] | Undefined, missing idle, process errors or timeout cannot be converted into a successful seat result. Default instruction/config/session discovery is extra context, not approved Roster scope. |
| Tool control | `--available-tools`, `--excluded-tools`, `--allow-tool`, `--deny-tool`, URL/path permission options. Do not use `--allow-all`, `--fleet`, autopilot or unbounded path/URL grants. [Programmatic][programmatic] | `defineTool` callbacks or raw JSON schemas; available/excluded tool filters, source-qualified custom/builtin/MCP selection. Same-name builtin overrides require explicit opt-in. Changing client's tools does **not** remove builtin/MCP/plugin/other-client tools. [Node reference][node] [Changing tools][changing] | A future adapter must select only Roster callbacks and validate arguments/scope again. Empty custom tools are not proof of no auto-tools. No shell/filesystem/network/MCP/GitHub defaults approved here. |
| Model discovery | `--model` selects a model; documented interactive model selection is not an entitlement check. [Programmatic][programmatic] | `listModels()` and `models.list` supply model capabilities/pricing information. Auto model routing also exists. [Node reference][node] [Usage][usage] | No auto fallback. Requested model is not proof of the model that served a call. No discovery invoked. |
| Cancellation/deadlines | Interactive Esc/Ctrl+C cancellation and shutdown shortcuts are documented. [Commands][commands] | `abort()` cancels currently processing message; `disconnect()` frees local resources but preserves disk state; deprecated `destroy()` means disconnect. `deleteSession()` deletes persisted data. `stop()` returns cleanup errors; owned stdio shutdown waits up to 10 seconds before termination; `forceStop()` exists. [Node reference][node] | A wait timeout is not proof of server cancellation, process exit, stopped callbacks or deletion of persisted data. A future worker needs explicit wall-clock cap, abort, unsubscribe, drain/stop, cleanup error handling and controlled state retention. |
| Budgets | A prompt may include multiple inference calls and tool steps. | `sessionLimits.maxAiCredits` is a **soft cap**, checked after calls; one response can exceed it. [Session limits][limits] | No universal concurrency/rate ceiling or guaranteed maximum bill inferred. Human spend policy must account for overshoot. |
| Per-call metrics | Text/stats or JSONL output is available, but this spike has no verified CLI per-inference-call model/token/cost capture contract. [Programmatic][programmatic] | `assistant.usage`: reported `model`, `inputTokens`, `outputTokens`, `cost`; ephemeral, not replayed on resume. Context utilization is a different event. Totals/context RPCs include experimental surfaces requiring pinning. [Usage][usage] [Events][events] | Capture actual per-call events, including sub-agent calls, before activity starts. Missing values stay unknown, not zero or configured-model fallback. Cost field is provider-specific, not automatically USD. |

The [usage guide][usage] describes `cost` as a premium request multiplier while
current [billing docs][billing] describe token/model-priced AI credits. Preserve
the reported field with its schema/version; do not reinterpret it as USD or
fabricate a credit-to-cost conversion for an uninspected account. A future
implementation must reconcile the pinned runtime's actual accounting surface.
Rate limits exist, but [limits documentation][rate] does not give a universal
requests-per-minute or concurrency number.

## Authentication, telemetry and physical containment

The [CLI authentication guide][cli-auth] documents:

- Precedence: `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, stored OAuth
  keychain token, then `gh auth token` fallback. Automatically injected
  Codespaces tokens have a documented precedence exception; explicitly
  exported tokens still override.
- OAuth through browser/device flow; fine-grained user PATs with Copilot
  Requests permission; App user-to-server tokens. Classic PATs are unsupported.
  Token expiration depends on the token/account route; do not assign one
  universal lifetime.
- OAuth storage: Windows Credential Manager, macOS Keychain, Linux libsecret;
  plaintext configuration fallback may be offered when the keychain is absent.
  `/logout` removes local stored authentication but does not revoke the token
  on GitHub. Revocation is a human-controlled separate operation.
- `COPILOT_OFFLINE=true` disables GitHub auth attempts and telemetry, but BYOK
  can still send prompts to a remote endpoint. That mode is **not** a
  subscription-backed offline inference route or automatically air-gapped.

The [SDK auth guide][sdk-auth] exposes `gitHubToken` and `useLoggedInUser: false`;
default runtime env inheritance is documented in the [Node reference][node].
An explicit env object, separate data directory and disabled stored-user
selection are design ingredients, **not physical isolation proof**.
Experimental in-process FFI shares the host process and rejects the
per-runtime env/telemetry/working-directory options: it is not an acceptable
publisher/inference credential separation shortcut.

The documented [organization server-to-server route][server-auth] needs
Copilot Requests repository permission **Read & write**, organization
enablement and the current **All repositories** installation scope. Installation
tokens expire after **one hour** and must be supplied through the runtime
`COPILOT_GITHUB_TOKEN` environment path, not the SDK's user-token option.
This is a separate human/org-authorized inference App design, not authorization
to repurpose or change Roster's publishing App.

**Unresolved physical boundary:** clearing env variables or redirecting
`COPILOT_HOME`/HOME cannot establish that a same-user process cannot reach the
physical home, OS credential manager, registry, mounted App key, inherited
handles, installed `gh`, cached tokens or credential helpers. Removing PATH
alone does not prevent execution by absolute path. `COPILOT_HOME` redirects
Copilot configuration/state ([directory guide][config]), not the entire OS
identity. No documented option evaluated here proves all these paths are
denied for real CLI execution. A dedicated OS principal/container/VM with
explicit filesystem/process/network grants and no accessible publisher
credential store requires independent validation. No real CLI auth-containment
acceptance is claimed; this is a **NO-GO** gate for live inference.

Prompt, attachment, code and tool-result data can reach the inference service;
session histories/config/logs may also persist locally. The
[OpenTelemetry guide][telemetry] documents opt-in runtime trace configuration,
OTLP export destinations and a `captureContent` option;
the CLI auth guide states telemetry can occur in BYOK mode unless offline mode
is enabled. A future worker must explicitly decide content capture, export
destinations, retention and policy before sending data. No provider telemetry
or entitlement inquiry occurred in this spike.

## What the fixture proves (and does not)

Only [the test helper](../tests/helpers/copilot-offline.mjs) and
[targeted tests](../tests/copilot-feasibility.test.mjs) implement the prototype.
There is **no import from production**, configuration switch, live adapter,
package/Node range change, SDK/CLI install, login or publisher invocation.

- Input is a small Roster-shaped text request with an exact existing
  `read_file` definition, explicit file scope, a 64 KiB payload bound, at most
  four transcript records and one scoped tool call.
- `assistant.message`, optional `assistant.usage` and `session.idle` use a
  deliberately restricted synthetic subset. `fixture.tool_request` is **our
  fake custom-tool handoff**, not a documented Copilot session event or
  wire-protocol replay. No UUID/session envelope or protocol compatibility
  claim is made.
- Request/fixture/scope snapshots prevent caller mutation during the fake
  wait. The real `createTools` validates scoped reads, path escapes and
  protected surfaces with `sliceReadsOnly: true`, `scopeExpansion: 0`,
  no test/internet permission and command/network tripwires.
- Results use Roster assistant/function-call/tool-message shapes,
  `prompt_tokens`/`completion_tokens` and completion/toolset/tool-result events.
  Tool results are returned to Roster, **not** fed into a second fake/model
  call. This tests mapping, not SDK control of its subsequent agent loop.
- Unknown request fields, events, malformed/unknown tools and arguments,
  duplicate/late records, invalid usage, empty completion, missing idle,
  oversize input and invalid deadline throw explicitly. Scoped tool failures
  also reject; no successful response is fabricated.
- Missing reported model is `null`, not the requested model. Missing usage is
  `null`; partial counts stay missing. Unknown cost is `null`, reported zero
  remains zero. Provenance is always `offline-fixture`, never publishable:
  even fully populated synthetic usage is not a real measured seat/eval.
- Parent env input is discarded into a frozen empty inference descriptor.
  No executable exists, and `gh` fallback is unavailable because **no process
  is launched**, not because a real auth path has been contained. Synthetic
  publisher context is unchanged. All token/App/HOME/PATH/config sentinels
  stay out of the descriptor.
- Pre-cancel starts nothing; mid-wait abort and deadline reject, clear the
  fake timer/listener and release synthetic lifecycle state. A tool failure
  also releases state. These flags are not observed SDK disconnect/stop
  calls. Cancellation of real in-flight tools, child trees or persistent
  sessions is **not** proven.
- Network fetch tripwire observes zero calls; the helper imports only Roster
  cancellation and tools and exposes no auth/model/publisher capability.
  It does not globally sandbox the test process or all production tools.

## Reproduction and measured evidence

Owned root: `D:\oss\github-agent-roster.worktrees\isolated-parallel-sessions-impl`.
Branch: `agents/isolated-parallel-sessions-impl`.
Clean stale generated branch fast-forwarded to reviewed main
`4de15f9558a117c183845e532e862664268fd9d2`.
Contracts submodule initialized at
`fa48c244bc9b8ead5df927ec705ef41a39c56f68`; `resolveContractsPath()` selected it.

No runtime was installed. Default host Node was v24.19.0; the existing
`C:\Users\harrr\AppData\Local\nvm\v20.19.5\node.exe` was used explicitly:

```powershell
& 'C:\Users\harrr\AppData\Local\nvm\v20.19.5\node.exe' --test `
  tests\copilot-feasibility.test.mjs `
  tests\tools.absolute-path.test.mjs tests\openai.test.mjs
```

Measured 2026-10-12: **39 tests, 39 pass, 0 fail, 0 cancelled, 0 skipped,
0 todo; TAP duration 2485.0884 ms**, exit 0. This includes 11 new fixture
tests and targeted existing transport/path regressions. No full-suite,
cross-platform, minimum Node 20.0, live model, SDK compatibility, spend or
real CLI sandbox claim follows from that run.

App environment names were present, but actual host-model provenance was not
independently verified. Consequently this child performed **no GitHub writes**,
commit, push or PR publication. Requested #570 Roster Build Inprogress status
was handed to the parent for a verified App-only write; this report does not
assert that the project status changed. #569 remains open. #571 and onboarding/
live calibration are out of scope.

### Parent technical review

After explicit exclusive-owner release, the parent independently read the
entire three-file scope and rechecked the pinned official SDK package/agent-loop
sources and current official CLI install/auth documentation. The measured
parent replay used Windows Node `v20.20.2` with the same three-file test selector:
**39/39 pass**, zero failures/skips, **2500.8772 ms**.

The technical outcome is the bounded offline GO / live-adapter NO-GO decision
above. Child Auto-selected model identity was unverifiable; parent review or
publication declarations must not be presented as measured child model/usage.
No installation, live inference, physical credential-isolation acceptance,
human AI-Eval or dependency exception follows from this review. #569 and the
#571-#573 decision/onboarding/live gates remain separate and open.

## Official sources

All checked 2026-10-12. Documentation evolves; recheck and pin before any
authorized implementation. URLs below are public, not account-specific probes.

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
