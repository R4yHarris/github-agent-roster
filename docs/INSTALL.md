# Build the native Rust application

This feature branch is a Rust rewrite, not the old Node/npm application.
There is no npm package entry point, parallel Node implementation or legacy
oracle checkout. Main and Git history retain the former application.

The native CLI implements the interactive shell (`roster` / `roster shell`),
interactive onboarding (`roster onboard [--discover]`), help, recipe
validation, catalog parsing, argument splitting, offline `init`/`doctor`
(including `doctor --copilot` and `doctor --openshell`), Ask and clarification
checkpoints (`ask`, `ask --pending|--cancel|--resume`), read-only repository
questions (`question`), fleet listing (`fleet list`), durable provenance
history (`history list|show`), bounded state cleanup (`clean`), encrypted file
vault (`vault set|list|get`), analytics and learning (`stats`, `learn --recurring`,
`recommend`, `eval`), Windows-native local TASK/Ask and fresh App-authenticated
issue delivery, including fixed-profile rescoping, fresh-context perspectives
and independent review repairs, plus the native NVIDIA OpenShell policy
compiler, formal boundary prover, fleet inference provider bridge and sandbox
attempt lifecycle (`rust/openshell.rs`). Top-level CLI GitHub publication,
resume and automatic fleet recovery/routing remain unavailable.

## Single-Command Installation (Recommended)

Install `github-agent-roster` and initialize your machine-scoped `~/.roster/`
configuration (`config.yml`, `fleet.yml`, `capabilities.yml`, `vault/`, and
`installation/`) with a single command:

**Linux, WSL 2, and macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/R4yHarris/github-agent-roster/main/install.sh | bash
```

**Windows PowerShell:**

```powershell
irm https://raw.githubusercontent.com/R4yHarris/github-agent-roster/main/install.ps1 | iex
```

The installer sets up `~/.roster/installation` (with `core.autocrlf=false` and
the pinned `vendor/github-agent-contracts` submodule), builds and installs the
native `roster` binary (`~/.local/bin/roster` on Unix/WSL;
`%USERPROFILE%\.roster\bin\roster.exe` and `%USERPROFILE%\.dotnet\tools\roster.exe`
on Windows), initializes machine-scoped `~/.roster/{config.yml,fleet.yml,capabilities.yml,vault}`,
and configures `ROSTER_MACHINE_HOME`, `ROSTER_INSTALLATION_ROOT`, `ROSTER_GIT`,
`ROSTER_NODE20`, and `ROSTER_CARGO`.

After installing, run `roster onboard --discover` once on your machine to
populate `~/.roster/config.yml` and `~/.roster/fleet.yml`. All workspaces on
the machine inherit `~/.roster/` automatically unless a workspace explicitly
defines its own `<repo>/.roster/` override.

## Manual Source Build

### Prerequisites

- Rust 1.85 or later with Cargo, rustfmt and Clippy.
- On Debian/Ubuntu Linux or WSL 2: `build-essential`, `pkg-config`, and `libssl-dev`.
- Git and the reviewed contracts submodule.
- Node 20 only for JavaScript consumer checks and the independent contracts
  identity/publishing SDK adapter, not as a Roster runtime or fallback.

## Windows PowerShell

```powershell
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
cargo fetch --locked
cargo build --release --locked --offline
.\target\release\roster.exe --help
```

Select this feature branch in the checkout when evaluating the rewrite.
`cargo fetch` restores the declared locked dependencies during setup;
ordinary build, lint and test commands remain offline.

## Linux, macOS and WSL

Install the Rust toolchain and linker prerequisites inside that environment,
then build the same Cargo package:

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
cargo fetch --locked
cargo build --release --locked --offline
./target/release/roster --help
```

Windows execution does not establish Linux/macOS/WSL support. Native platform
verification and binary distribution remain release gates.

## Verification and resources

See [TESTING](TESTING.md) for formatting, strict Clippy, native unit/integration
tests, minimum-compiler checks and external consumer fixture requirements.
Use a persistent source checkout while resource installation and the remaining
CLI are being implemented. This branch does not claim a completed packaged
installation, model onboarding, cross-platform delivery or the complete
issue-to-publication delivery loop.

## Explicit local host configuration

`ROSTER_INSTALLATION_ROOT`, when set, must be an absolute native development
installation containing skills, principals, guidance and the reviewed contracts
gitlink. The current development binary otherwise uses its build-time checkout.
Resource bundling/relocatable release installation is still pending.

For Windows local delivery, choose installed images explicitly:

```powershell
$env:ROSTER_GIT = 'C:\Program Files\Git\cmd\git.exe'
$env:ROSTER_NODE20 = 'C:\approved-node20\node.exe'
.\target\release\roster.exe doctor
.\target\release\roster.exe run --seat coder --runtime builtin
.\target\release\roster.exe run --ask-file ASK.md --runtime builtin --no-fetch
.\target\release\roster.exe run --issue 42 --runtime builtin --no-fetch
```

These paths are examples, not executable discovery or installation commands.
Only select images actually installed and approved on the host. Node is an
optional consumer-language engine and the explicit contracts identity adapter,
never a Roster fallback. Issue mode additionally needs complete App credentials
with the private key outside source repositories; it refuses existing issue
claims rather than resuming them. See [fresh issue delivery](RUST.md#native-fresh-issue-delivery).

For a Rust consumer set an absolute `ROSTER_CARGO`, the installed toolchain's
`CARGO_HOME`/`RUSTUP_HOME` and, where needed, `RUSTUP_TOOLCHAIN` and `RUSTFLAGS`.
Set `CARGO_TARGET_DIR` to owned output **outside** the consumer worktree.
Cargo checks run formatting, strict all-target Clippy and locked/offline
all-target tests. Roster does not acquire a compiler, packages or images.
Mixed written Node/Rust scope needs both effective engines.

Both local run forms use the same bounded fixed-profile continuation loop:
at most two scope rescopings, two fresh-context perspectives and two completed
semantic review repairs. Correct source edits and the earliest baseline stay
in the same worktree/writer lease. Each repair gets a new read-only reviewer;
malformed/incomplete reviews, policy denials, cancellation and source/config
drift cannot authorize retries. A fresh context does not reset the whole-task
deadline or `llm.max_requests`. There is no silent model switch.

Expansion budgets grow only from actual recorded unprotected blocked paths,
up to 16. A strict zero remains zero. Exhausted scope policy prints
`Mode: planning-escalation`, returns a non-success exit and requires a later
explicit human/planner handoff; it does not rewrite TASK or claim acceptance.
Summaries list actual continuation counters, scope expansions and owned
artifact archive paths. Existing TASK/RECIPE/ESTIMATE identity is preserved.
`--skip-review` still requires private `reviewer.required: false` and never
claims publishable independent review. Unsupported routing, resume, issue and
publication flags remain rejected before effects.

`roster init` creates only `roster.config.example.yml` and
`ROSTER-POLICY-NOTE.md`, keeping existing regular files. It does not create
private configuration, ignore rules, credentials, `agent-policy.yml` or
workflows. Before a local run, a human must review ignore rules for the
runtime/private subpaths under `.roster/` (not a blanket rule hiding tracked
shared setup), the configured worktree directory and managed root artifacts (ASSIGNMENT,
TASK, RECIPE, ESTIMATE, CONTEXT, RESULT, REVIEW, PLAN and RESEARCH).

### Configuration and environment

Target canonical repository private config precedes installation private
config; linked worktrees do not acquire a separate private overlay. With no
required private file, embedded, parsed defaults select an offline stub.
Malformed, unreadable, linked, multi-link or changed private settings fail,
never fall back to defaults. Unix private directories/files require 0700/0600;
Windows path/identity checks are not a claim of a new ACL provider.

| Host override | Meaning |
| --- | --- |
| `ROSTER_LLM_BASE_URL`, `ROSTER_LLM_MODEL` | Explicit endpoint/model overrides; a configured endpoint needs a model. |
| `ROSTER_LLM_API_KEY_ENV` | Name of the selected environment key, not its value; App credential names are refused. |
| `ROSTER_LLM_REQUEST_TIMEOUT_MS` | Validated native request timeout override. |
| `ROSTER_TASK_TIMEOUT_MS` | Whole native task cap, 1–2147483647 ms. |
| `ROSTER_START_BASE`, `ROSTER_START_SYNC` | `trunk`/`current`/tracked ref or complete SHA; `fetch`/`offline`. |
| `ROSTER_ALLOW_FETCH` | `true` explicitly permits credential-free HTTP(S) origin fetch; default is deny. |
| `ROSTER_BACKEND_TIMEOUT_MS` | Host backend cap, 1–900000 ms; default 30000. Hook and matching-check guards still apply. |
| `ROSTER_LOCAL_SUBMODULES` | At most 16 explicit local-source approvals, JSON described below. |

Blank or malformed selected overrides fail. No command changes the process's
global environment or working directory. Keys named by the selected LLM or
named critic profile are read only from the injected invocation environment.
Selected private critic fleet settings are validated and sealed with the config.
There is no automatic cloud discovery or real vault-provider claim.

An omitted critic `api_key_env` is valid. The effective configuration is resolved
through the existing fleet-profile semantics: explicit profile key name first,
otherwise the configured `vllm-local` key-name fallback; optional auth follows
the inherited LLM preference or that named local-profile default. Only those
effective selected environment names are read. Optional keyless local profiles
remain usable; missing required, blank or invalid selected credentials fail.
Selected values are secret regardless of names such as `CRITIC_AUTH`. No
credential value is written to private settings, managed artifacts or logs.
Timeout/stall/callback diagnostics conceal normalized primary and critic
authorities, not just the full configured URL.

Retention overrides retain the existing `ROSTER_RETENTION_OPT_OUT` boolean
and positive `ROSTER_RETENTION_WINDOW_MS_{MACHINE,REPO,WORKTREE,RUN}` contract.
Defaults are 90, 14, 7 and 1 days respectively. Loading these preferences does
not enable cleanup or silently delete history.

### Worktrees and dependencies

An Ask-file run creates a new Git worktree and host-generated opaque local ID;
it does not copy the caller's dirty working files. Trunk resolves origin's
default ref, then origin/main or origin/master; absent origin/default refs
fall back to current HEAD with freshness explicitly unknown. Explicit refs
must be unambiguous existing tracked refs or complete commit SHAs. Existing
worktrees and branches are never reset, force-moved or removed.

No preparation fetches `.gitmodules` URLs. Approve each declared gitlink's
absolute local source explicitly, including nested paths:

```powershell
$env:ROSTER_LOCAL_SUBMODULES = '[{"path":"vendor/github-agent-contracts","source":"D:\\approved\\github-agent-contracts"}]'
```

Every source HEAD must exactly match its captured gitlink. Materialization is
Git-only, local, bounded and pinned; the source pack and `.gitmodules` are not
rewritten. Omitted, extra, stale or nonlocal approvals fail. Production origin
fetch and local submodule materialization are separate permissions.

The first checkpoint uses the strict Windows owned-process backend. Strict
Unix/macOS Git/engine execution remains unavailable rather than silently
falling back to direct-child ownership. Pure leaves and offline initialization
do not imply cross-platform delivery acceptance.

Initialize an existing checkout's contracts dependency from its own root:

```powershell
git submodule update --init --recursive
```

The native [contracts resolver](../rust/paths.rs) checks the initialized
submodule, `GITHUB_AGENT_CONTRACTS`, then the sibling clone; every candidate
must contain `scripts/agent-pr.mjs`. Do not copy or edit vendor sources.
See [DEPENDENCY](DEPENDENCY.md) for App-only publishing and policy boundaries.
