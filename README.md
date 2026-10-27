# github-agent-roster

**github-agent-roster** is a native Rust software-delivery orchestrator.
GitHub Issues and Pull Requests are the board and the forge. Planning, task
artifacts (`RECIPE.yml`, `TASK.md`, `PLAN.md`, `ESTIMATE.md`), sequential
builtin seats (`planner` $\rightarrow$ `coder` $\rightarrow$ `reviewer`),
guarded tools, skills, curated memory, and isolated Git worktrees belong to
Roster; cryptographic bot identity, policy verification, commit trailers, and
PR publication belong to the independent
[github-agent-contracts](vendor/github-agent-contracts) SDK. There is no Kanban
database, external worker swarm, or Node application runtime fallback.

---

## Quickstart & Single-Command Installation

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

*(Or run `./install.sh` / `.\install.ps1` directly from a local checkout.)*

What the single-command installer does automatically:
1. Installs any missing OS and toolchain prerequisites (`build-essential`, `pkg-config`, `libssl-dev`, `git`, `curl`, `rustup`, and Node 20 via `nvm` on Linux/WSL 2/macOS; Rust toolchain and Node 20 detection on Windows).
2. Clones `github-agent-roster` (with the pinned `vendor/github-agent-contracts` submodule and `core.autocrlf=false`) into `~/.roster/installation` and compiles the native release `roster` binary into `~/.local/bin/roster` and `~/.roster/bin/roster` (plus `%USERPROFILE%\.dotnet\tools\roster.exe` on Windows).
3. Initializes machine-scoped configuration in `~/.roster/` (`config.yml`, `fleet.yml`, `capabilities.yml`, `vault/`) with owner-only (`0700`/`0600` or Windows ACL) permissions and persists `ROSTER_MACHINE_HOME`, `ROSTER_INSTALLATION_ROOT`, `ROSTER_GIT`, `ROSTER_NODE20`, and `ROSTER_CARGO` in `~/.roster/env.sh` (sourced by `~/.bashrc`/`~/.zshrc`) or Windows `HKCU:\Environment`.
4. Runs `roster doctor` and `roster fleet list` to confirm readiness.

### Machine-Scoped `~/.roster/` Configuration

Your LLM fleet, default model configuration, capability priors, and API keys
belong to your **machine**, not to individual repositories:

- **`~/.roster/config.yml`** — Machine-wide default LLM endpoint, model, and seat preferences.
- **`~/.roster/fleet.yml`** — Machine-wide GPU/endpoint fleet catalog (`default`, `rtx-3090-qwen`, `aperture-qwen`, `aperture-glm`, `aperture-dsv4`, `spark-dsv41`, etc.).
- **`~/.roster/capabilities.yml`** — Machine-wide model capability priors overlay.
- **`~/.roster/vault/`** — Machine-wide AES-256-GCM encrypted API keys (`0700`/`0600`).
- **`~/.roster/installation/`** — Machine-wide Roster runtime assets (`principals/`, `skills/`, `examples/`, and `vendor/github-agent-contracts`).

Because these live in `~/.roster/`, you can `cd` into **any** repository or
workspace on your machine and immediately run `roster`, `roster fleet list`,
`roster doctor`, or `roster run` without copying `.roster/fleet.yml` or
re-running `roster onboard` in each workspace. (If a workspace explicitly
creates its own `<repo>/.roster/config.yml` or `<repo>/.roster/fleet.yml`,
that workspace file overrides the machine default for that repository only.)

Run `roster` with no arguments in an interactive terminal to open the
**Interactive Roster Shell**:

```text
github-agent-roster  v0.2.1
node 20 · powershell · D:\oss\github-agent-roster · main
endpoint configured · qwen3.8-27b · 127.0.0.1:8000
contracts v0.2.1-6-g0050ada · update skipped · warnings none
───────────────────────────────────────────────────────────────────────────────
idle │ qwen3.8-27b m │ - / 262.1k │ -
───────────────────────────────────────────────────────────────────────────────
roster>
```

- **Startup banner**: displays product version, runtime/shell, working directory,
  Git branch, LLM endpoint state/model/host, contracts submodule version, and
  any health warnings.
- **Pinned status rail**: sits between two cyan rules directly above `roster> `,
  showing the active issue/phase (`idle`, `plan`, `draft`, `test`, `review`,
  `pass`, `fail`), configured model + effort letter (`l`, `m`, `h`, `x`),
  server-reported token context bar (`12.4k/262.1k [####------] 5%`), and
  elapsed time.
- **Natural-language Asks**: type an Ask directly at `roster> ` (or prefix with
  `/ask `) to classify (`clarify` | `slice` | `feature` | `initiative`), draft
  `RECIPE.yml` + `TASK.md` (or `PLAN.md`), or answer scoped read-only repository
  questions.

---

## CLI Command Inventory & Status

All CLI subcommands below are implemented in the native Rust `roster` binary
([`rust/main.rs`](rust/main.rs), [`rust/local_cli.rs`](rust/local_cli.rs)):

| Command | Status | Description & How to Use |
| --- | --- | --- |
| `roster` / `roster shell` | **Working (Native Rust)** | Open the interactive Roster shell in a TTY with banner, pinned status rail, history, and `/`-command dispatcher. |
| `roster --debug [COMMAND]` | **Working (Native Rust)** | Enable process-only redacted debug JSONL logging (`.roster/logs/debug-<session>.jsonl`) for the shell or command. |
| `roster --help` / `-h` | **Working (Native Rust)** | Print CLI usage and command summary (also printed when `roster` is invoked with non-TTY stdin and no arguments). |
| `roster init` | **Working (Native Rust)** | Exclusively create `roster.config.example.yml` and `ROSTER-POLICY-NOTE.md` without overwriting existing files or `agent-policy.yml`. |
| `roster update [--check]` | **Working (Native Rust)** | Check for or install the latest Roster release from `origin/main` in `~/.roster/installation`, update the pinned contracts submodule, rebuild the native binary, and safely replace installed binaries on Windows and Unix/WSL. |
| `roster onboard [--discover]` | **Working (Native Rust)** | Interactive 5-step terminal wizard (`Platform`, `LLM endpoint`, `Permissions`, `Review`, `Doctor`) that probes `/v1/models` and writes `.roster/config.yml` and `.roster/fleet.yml`. |
| `roster ask "TEXT"` | **Working (Native Rust)** | Classify an Ask (`clarify`, `slice`, `feature`, `initiative`). Creates a GitHub issue when `gh` is available (deriving a concise $\le 240$-char title for long single-line prompts while keeping the full Ask in the body), saves an offline draft when `gh` is unavailable, or saves a private clarification checkpoint when scope is missing. |
| `roster ask --pending` | **Working (Native Rust)** | Inspect the pending private clarification checkpoint (`.roster/asks/clarification.json`), including assumptions and clarifying questions. |
| `roster ask --resume ["TEXT"]` | **Working (Native Rust)** | Merge follow-up scope/Allowed Files into the pending clarification checkpoint and resubmit. |
| `roster ask --cancel` | **Working (Native Rust)** | Atomically remove the pending clarification checkpoint without starting execution. |
| `roster question [--files PATH,PATH --] TEXT` | **Working (Native Rust)** | Answer a read-only repository question using only public source excerpts matching the explicit file allowlist; never starts a task or writes files. |
| `roster doctor` | **Working (Native Rust)** | Offline check of repository configuration, approved `ROSTER_GIT`/`ROSTER_NODE20`/`ROSTER_CARGO` images, and initialized `vendor/github-agent-contracts` submodule. |
| `roster doctor --copilot [--opt-in] [--runtime PATH] [--model ID]` | **Working (Native Rust)** | Offline Copilot worker capability and blocker assessment; never launches a worker or network login. |
| `roster doctor --openshell [--opt-in] [--runtime PATH] [--gateway URL] [--policy PATH] [--provider ID]` | **Working (Native Rust)** | Offline NVIDIA OpenShell sandbox, policy, and inference provider readiness diagnostics; never opens a gateway or network connection. |
| `roster fleet list` | **Working (Native Rust)** | Read and format validated `.roster/fleet.yml` profiles (`ID`, `MODEL`, `BASE_URL`, `CONTEXT_MAX`, `CONCURRENCY`, `HARDWARE`, `TASK_CLASS`) with secret redaction. |
| `roster history list [FLAGS]` | **Working (Native Rust)** | Query durable provenance records (`--format json\|text\|summary`, `--store DIR`, `--repo HASH`, `--issue N`, `--seat NAME`, `--model MODEL`, `--outcome VALUE`, `--since TIME`, `--until TIME`). |
| `roster history show <ID> [FLAGS]` | **Working (Native Rust)** | Inspect a single provenance record, run, or session in `text`, `json`, or `summary` format. |
| `roster clean [FLAGS]` | **Working (Native Rust)** | Dry-run-by-default cleanup of `.roster` / `.roster-state` and provenance stores (`--target issue\|repo\|machine-history\|curated-memory`, `--issue N`, `--scope SCOPE`, `--state-root PATH`, `--exclude NAME`, `--execute --yes`). Refuses active run locks and forbidden paths. |
| `roster bench` | **Working (Native Rust)** | Measure and write `.roster/bench.json` with six secret-free timings (`process_start_ms`, `config_load_ms`, `command_dispatch_ms`, `git_worktree_add_ms`, `submodule_init_ms`, `mocked_round_trip_ms`). |
| `roster status --issue N [--offline] [--json]` | **Working (Native Rust)** | Read bounded per-worktree session log state offline, or combine with App-authenticated GitHub issue metadata online. |
| `roster stats [--delivery] [--json] [--ref REV] [--evals PATH]` | **Working (Native Rust)** | Join Git `AI-Run` history, local run JSONL records, and human `AI-Eval` decisions into model/seat summaries or `--delivery` attempt/hardware tables, plus open failure proposals. |
| `roster learn --recurring` | **Working (Native Rust)** | Synthesize human-reviewable draft failure proposals under `.roster/proposals/` from recurring failed/rejected slices without mutating active rules. |
| `roster recommend --task-class CLASS [--difficulty 1-5] [--json]` | **Working (Native Rust)** | Rank coder models from qualifying human evaluations and earned difficulty ceilings (`feat`, `fix`, `docs`, `test`). |
| `roster eval TARGET VERDICT 1-5 y\|n --local\|--post [FLAGS]` | **Working (Native Rust)** | Record a human `AI-Eval` decision (`accept`, `reject`, `rework`) locally in `.roster/evals.jsonl` or post a verified human PR comment (`--post`). |
| `roster vault set\|list\|get NAME` | **Working (Native Rust)** | Manage AES-256-GCM encrypted secrets in `~/.roster/vault` with OS-enforced owner-only ACLs (`0700`/`0600`). `set` reads from piped stdin; `get` writes to piped stdout. |
| `roster run --seat coder --runtime builtin [--skip-review]` | **Working (Native Rust)** | Execute an existing `TASK.md` in the current worktree through the bounded builtin coder and fresh read-only reviewer. |
| `roster run --ask-file PATH --runtime builtin [FLAGS]` | **Working (Native Rust)** | Prepare an isolated `agents/local-<id>` worktree and run sequential builtin `planner` $\rightarrow$ `coder` $\rightarrow$ `reviewer` seats (`--base`, `--no-fetch`\|`--fetch`, `--skip-review`). |
| `roster run --issue N --runtime builtin [FLAGS]` | **Working (Native Rust)** | Authenticate as the GitHub App, read open issue `#N`, seal the caller index, prepare an isolated `agents/N` worktree, and run sequential builtin seats. |
| `roster recipe validate PATH` | **Working (Native Rust)** | Validate strict v0 `RECIPE.yml` seat definitions, read/write roots, and tool allowlists. |
| `roster catalog parse ROOT FIELD...` | **Working (Native Rust)** | Parse and format restricted YAML catalogs from stdin. |
| `roster arguments split` | **Working (Native Rust)** | Split shell-quoted argument strings into a JSON array. |
| `roster run --publish` / `--resume` / `--parallel` / `--attempts` | **Planned / Library-Ready** | Helper logic is implemented in [`rust/run.rs`](rust/run.rs) and [`rust/start.rs`](rust/start.rs); top-level CLI flags fail closed until full end-to-end App publication/resume cutover. Use `/publish` in the shell or `agent-pr.mjs` directly. |

---

## Interactive Shell (`/`) Command Inventory & Status

Inside `roster` (or `roster shell`), all 38 slash commands across 6 groups are
available ([`rust/shell.rs`](rust/shell.rs)):

### 1. Session Commands

| Command | Aliases | Status | Description & Example |
| --- | --- | --- | --- |
| `/help [GROUP\|COMMAND]` | `/` | **Working** | Show all command groups (`Session`, `Ask`, `Model`, `Board`, `Human`, `Settings`) or a detailed command page (`/help fleet`). |
| `/status [N] [--offline]` | — | **Working** | Show current session state (`Issue`, `Branch`, `Seat`, `State`, `Model`, `Host`, `Effort`, `Review`, `Worktree`) or query issue `#N`. |
| `/history` | — | **Working** | Display the last 20 secret-scrubbed commands from `.roster/history`. |
| `/context` | — | **Working** | Show last seat context readout (`Seat`, `Provider`, `Model`, `Effort`, `Input`, `Output`, `Context max`, `Finish reason`, `Pack budget`). |
| `/usage` | — | **Working** | Show token usage, cached tokens, tool calls, elapsed time, and thinking state. |
| `/compact [--aggressive]` | — | **Working** | Two-stage context compaction: prunes bulky older tool outputs and condenses multi-turn conversation history into a rolling digest while preserving byte-stable system prompt caching. |
| `/paste [list\|clear\|drop N\|PATH]` | — | **Working** | Capture an image or text from the system clipboard (Windows PowerShell, WSL 2, macOS `pngpaste`/`pbpaste`, Linux `wl-paste`/`xclip`), attach a local file path (`#1`, `#2`, ...), or list/drop/clear active session attachments. Dragging or pasting a file path directly at `roster> ` also attaches it automatically for immediate or later turn reference. |
| `/copy [code\|N]` | — | **Working** | Copy the last assistant response, last fenced code block (`/copy code`), or Nth turn (`/copy N`) to the system clipboard (with OSC 52 fallback for remote terminals). |
| `/export [PATH]` | — | **Working** | Export the current multi-turn session conversation, attachments, and routing telemetry to a redacted Markdown file (`ROSTER_SESSION_EXPORT.md`). |
| `!<command>` | — | **Working** | Execute a local shell command in the current working directory and record its redacted output in session context. |
| `/clear` | — | **Working** | Clear the terminal screen, reset loaded subdirectory context-hint tracking, and redraw the pinned status rail. |
| `/redraw` | — | **Working** | Repaint the status rail and prompt at the current terminal width. |
| `/quit` | `/q`, `exit` | **Working** | Exit the interactive shell cleanly. |

### 2. Ask & Task Commands

| Command | Aliases | Status | Description & Example |
| --- | --- | --- | --- |
| `/ask TEXT` (or bare `TEXT`) | — | **Working** | Classify and plan an Ask in `.roster/asks/` with live streaming (`phase`, `tool`, `delta`, `wait`, `verdict`) and live pinned status rail updates, printing the `Task summary` (`Outcome`, `Allowed files`, `Checks`), artifact paths, and `roster run --ask-file` command, or saving a clarification checkpoint. |
| `/question [--files P --] TEXT` | — | **Working** | Answer a read-only scoped repository question without creating a worktree or writing files. |
| `/btw QUESTION` | — | **Working** | Ask a single-turn, tool-free side question about the active task (`TASK.md`, `PLAN.md`, `REVIEW.md`). |
| `/clarify [show\|cancel\|continue\|TEXT]` | — | **Working** | Inspect, cancel, or resume a pending private clarification checkpoint with explicit Allowed Files. |
| `/plan TEXT` | — | **Working** | Stream planner drafting for `TEXT` (`TASK.md` / `RECIPE.md` / `PLAN.md`) and pause for `Enter` to accept and continue or `/stop` to keep the plan without coding. |
| `/queue [TEXT\|list\|drop N\|clear]` | `/qs` | **Working** | Manage the 8-slot session Ask queue (displayed above the top status rule). Pressing Enter on an empty line pops and runs the next queued Ask. |
| `/remediate [TEST ...]` | — | **Working** | Build a test-only remediation task and select the largest-context fleet profile. |
| `/publish [SUBJECT] [--model ID] [--skip-review]` | — | **Working** | Format the exact `AI_*` metadata environment and `agent-pr.mjs --merge-when-green` publication command for the reviewed worktree. |
| `/map` | — | **Working** | Generate a bounded ($\le 80$ lines) filename-only `.roster/map.md` in the active worktree and register it in `.git/info/exclude`. |
| `/checkpoints` | — | **Working** | List recorded product checkpoints (`number \| seat \| status \| time`) in the active worktree. |
| `/rewind N` | `/undo` | **Working / Scoped** | Rewind product files to checkpoint `N` (or latest with `/undo`) when an active task scope is loaded. |
| `/resume [N] [--green]` | — | **Working** | Inspect and attach to a local issue worktree (`.worktrees/issue-N`) and load its assignment/review state. |
| `/recap` | — | **Working** | Print a one-line recap (`Outcome \| Files \| Last test \| Review \| Finish reason`) from the active worktree's artifacts. |
| `/worktrees` | — | **Working** | List registered issue worktrees (`path \| branch \| seat \| status`) from `git worktree list`. |
| `/stop` | — | **Working** | Cancel an active run or pending confirmation. |
| `/steer TEXT` | — | **Scoped** | Interrupt an in-flight drafting coder model call with additional instructions within current `TASK.md` scope. |
| `/run` / `/retry` | — | **Working** | Execute or retry local Ask delivery (`delivery::Driver::run_local` with live streaming `phase`, `tool`, `delta`, `wait`, `verdict`) for the active planned Ask; `/run N` / `/review` bridge to `roster run` for issue worktrees. |
| `/batch` | — | **Refused by Design** | Always refuses (`One seat at a time. Worktrees are isolated.`). |

### 3. Model & Fleet Commands

| Command | Aliases | Status | Description & Example |
| --- | --- | --- | --- |
| `/model [ID\|clear] [--save]` | — | **Working** | Show or change the active session model (pass `--save` to persist to `.roster/config.yml`). |
| `/effort [l\|m\|h\|x\|none\|status]` | — | **Working** | Show or change the session reasoning effort level (`l`, `m`, `h`, `x`, `none`) and update the rail. |
| `/provider` | — | **Working** | Show the active session profile ID and endpoint host without exposing credentials. |
| `/fleet [list\|use ID\|probe\|add\|discover\|hardware\|default\|remove]` | — | **Working** | List fleet profiles, switch the session profile (`/fleet use ID`), probe `/v1/models`, discover local endpoints, or add/update/remove profiles in `.roster/fleet.yml`. |
| `/route [on\|off\|auto\|status]` | — | **Working** | Inspect or configure hardware-aware NLP complexity classification (`fast`, `standard`, `deep`) and automatic fleet routing across `~/.roster/fleet.yml`. |
| `/stats [REF] \| /stats --delivery [--json]` | — | **Working** | Display model/seat aggregates, failure proposals, or `--delivery` metrics. |

### 4. Board Commands

| Command | Aliases | Status | Description & Example |
| --- | --- | --- | --- |
| `/issues` | — | **Working** | List up to 100 open GitHub issues (`#N title`) for the `origin` repository. |
| `/issue N` | — | **Working** | Show cached or live `Issue`, `State`, `Branch`, and `PR` summary for issue `#N`. |
| `/diff` | — | **Working** | Print tracked filenames changed relative to `HEAD` in the active worktree. |
| `/waves [open]` | — | **Working** | Read and display child wave drafts (`wave:N \| title \| state`) from `PLAN.md`. |
| `/log N\|debug` | — | **Working** | Tail redacted per-worktree seat logs for issue `#N` or the process debug JSONL log (`/log debug`). |

### 5. Human Feedback & Learning Commands

| Command | Aliases | Status | Description & Example |
| --- | --- | --- | --- |
| `/eval SESSION VERDICT 1-5 y\|n --minutes N --note "TEXT"` | — | **Working** | Record a human `AI-Eval` decision (`accept`, `reject`, `rework`) with required note on `reject`/`rework` or `again=n`. |
| `/recommend [feat\|fix\|docs\|test] [--difficulty 1-5]` | — | **Working** | Show advisory coder model recommendation from human evaluation history. |
| `/learn --recurring` | — | **Working** | Generate draft failure proposals in `.roster/proposals/` from recurring slice failures. |

### 6. Settings & Vault Commands

| Command | Aliases | Status | Description & Example |
| --- | --- | --- | --- |
| `/statusbar on\|off` | — | **Working** | Toggle the pinned status rail and rules for the current shell process. |
| `/debug on\|off\|status` | — | **Working** | Toggle or inspect process-only debug JSONL logging (adds `*` and `thinking · max_tokens` detail to the rail). |
| `/doctor [warm\|copilot\|openshell [FLAGS]]` | — | **Working** | Run offline prerequisite checks (`/doctor`), probe `/v1/models` with the configured request timeout (`/doctor warm`), or run offline Copilot/OpenShell diagnostics (`/doctor copilot`, `/doctor openshell`). |
| `/config [path\|set KEY VALUE]` | — | **Working** | Display redacted `.roster/config.yml`, print its path (`/config path`), or update `effort`, `context.budget`, `statusbar`, or `debug`. |
| `/vault [list\|get NAME\|set NAME]` | — | **Working** | List stored secret names, verify presence without printing the secret (`/vault get NAME`), or prompt with hidden input (`/vault set NAME`). |

---

## Build and check

Use Rust 1.85 or later, Git and the initialized contracts submodule:

```powershell
git submodule update --init --recursive
cargo build --release --locked --offline
cargo run --locked --offline -- --help
cargo fmt --all -- --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --all-targets --locked --offline
```

Dependencies must first be restored from the committed manifest/lock when
missing; ordinary verification is offline. Tests exercising JavaScript consumer
projects use an explicit Node 20 executable via `ROSTER_TEST_NODE`. That is
test/tool coverage, not a Roster implementation. The native CLI integration
tests run the pure CLI leaves and offline initialization with an empty
executable search path. Local Git/engine operations require explicit approved
images; they do not adopt commands from `PATH`.

## First local delivery

Use a persistent native development checkout, initialized at its reviewed
contracts gitlink. Set `ROSTER_GIT` to an absolute installed Git executable.
`ROSTER_NODE20` selects Node 20 for JavaScript **consumer** checks;
`ROSTER_CARGO` selects Cargo for Rust checks. Missing selected engines are
unavailable, not a reason to substitute another runtime.

Private `<canonical-target-repository>\.roster\config.yml` wins over the
installation's private config. Linked worktrees share the canonical private
settings. Without either file, validated built-in defaults select a truthful
offline stub, not a model. Invalid/private-linked/unreadable settings fail.
Configured runs need both an endpoint and model; credentials come only from
explicit environment-key names or the selected native file vault. Vault
storage-ownership acceptance remains open; no missing key becomes anonymous auth.

Existing-TASK mode requires clean product source, a strict bounded TASK with
its unchanged Original Ask, and pre-existing ignore rules for managed
artifacts/state. It never moves that worktree. Ask-file mode preserves dirty
human source and other worktrees while creating `agents/local-<opaque-id>`
in the configured worktree directory. Use `--no-fetch` for offline work;
production fetch also requires explicit trusted-host approval.

The same invocation holds one writer from pre-plan hooks through planner,
coder and fresh read-only reviewer. A verified exit requires current private
check/result/review receipts, not a model's `pass` text. Stub and planning-only
outputs explicitly do **not** claim delivery acceptance. Existing foreign
artifacts are preserved and refused; automatic resume/repair is not shipped.
No local mode publishes, creates a fake GitHub issue or closes one.

Issue mode requires complete App credentials, approved Git/Node20 and the
reviewed contracts dependency. It seals the original calling repository before
the App metadata await and refuses drift before preparation. Closed issues,
existing canonical/legacy/attempt branches, known remote claims and occupied
worktree paths stop rather than being adopted or replaced; native resume is
still unavailable. A successful configured run uses the same private
verification/review gates as local delivery. A stub does not edit code or claim
tests. No issue or PR write, queue-status update, publication or human
acceptance is implied.

See [installation and host configuration](docs/INSTALL.md),
[OpenShell feasibility](docs/OPENSHELL_FEASIBILITY.md),
[OpenShell architecture](docs/OPENSHELL_DESIGN.md), and the exact
[local-delivery contract](docs/RUST.md#native-local-delivery-checkpoint).

## Installing & Onboarding in a Consumer Repository / WSL 2 Workspace

When using `github-agent-roster` across one or more consumer repositories (for
example in WSL 2 `Ubuntu` under `~/repos/<your-repo>` or on Windows under
`D:\oss\<your-repo>`) that already use `github-agent-contracts` and the GitHub
App plugin, you install Roster and configure your fleet **once per machine**
in `~/.roster/`:

### Step 1: Run the Single-Command Machine Installer

**Inside WSL 2 (`wsl -d Ubuntu`), Linux, or macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/R4yHarris/github-agent-roster/main/install.sh | bash
source "$HOME/.roster/env.sh"

# (Optional) Install NVIDIA OpenShell CLI + local gateway in WSL 2 / Linux
curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | sh
```

**Inside Windows PowerShell:**

```powershell
irm https://raw.githubusercontent.com/R4yHarris/github-agent-roster/main/install.ps1 | iex
```

### Step 2: Configure Your Machine Fleet & Vault Once (`~/.roster/`)

Run the interactive onboarding wizard (or `roster fleet add` / `roster vault set`)
once on the machine. Everything is saved under `~/.roster/` (`config.yml`,
`fleet.yml`, `capabilities.yml`, and `vault/`), so every repository on the
machine inherits your setup automatically:

```bash
# Run the interactive 5-step onboarding wizard (Platform -> LLM endpoint -> Permissions -> Review -> Doctor)
roster onboard --discover

# Or add/manage machine-wide fleet profiles and encrypted API keys directly:
roster fleet list
roster fleet discover --wsl --save
roster vault set APERTURE_API_KEY
```

> **Note on WSL 2 $\leftrightarrow$ Windows / LAN Endpoints:**
> - Remote/LAN endpoints (`https://aperture.xen.blackmesaresearch.ai/v1`, `http://192.168.1.48:8000/v1`) work identically from Windows and WSL 2.
> - For a vLLM/Ollama server running on the Windows host (port `11435`), if WSL 2 is not in `networkingMode=mirrored`, use the Windows host IP (`ip route show | awk '/default/ {print $3}'`, e.g. `http://172.x.x.1:11435/v1`).
> - **Library usage (`Cargo.toml`)**: if your consumer workspace is also a Rust crate and wants to call `github_agent_roster::openshell` or orchestration APIs directly as a library, add `github-agent-roster = { path = "/home/<user>/.roster/installation" }` (or git dependency) to the consumer's `Cargo.toml`.

### Step 3: Prepare & Verify Any Consumer Repository (`~/repos/<your-repo>`)

Change into your consumer repository (which already has
`vendor/github-agent-contracts` or `GITHUB_AGENT_CONTRACTS` and
`agent-policy.yml`). No `.roster/fleet.yml` or `.roster/config.yml` copy is
needed in the workspace:

```bash
cd ~/repos/<your-repo>

# 1. Ensure your consumer .gitignore excludes Roster task state and untracked task artifacts:
cat << 'EOF' >> .gitignore
.roster/
.roster-state/
.worktrees/
TASK.md
RECIPE.yml
ESTIMATE.md
RESULT.md
REVIEW.md
PLAN.md
ASSIGNMENT.md
CONTEXT.md
RESEARCH.md
EOF

# 2. (Optional) Initialize example templates (never overwrites agent-policy.yml)
roster init
```

### Step 4: Verify & Test the Installation in Your Consumer Repo

Run this 5-step verification checklist from inside `~/repos/<your-repo>`:

```bash
# 1. Verify offline prerequisites, approved tool images, and contracts submodule resolution
roster doctor

# 2. Verify your machine-scoped ~/.roster/fleet.yml catalog from inside the consumer repo
roster fleet list

# 3. Verify NVIDIA OpenShell readiness (offline or with --approve-live)
roster doctor --openshell --opt-in --gateway http://127.0.0.1:8080 --provider aperture-qwen

# 4. Run a read-only scoped repository question against a file in your consumer repo
roster question --files README.md -- "Summarize the repository purpose in one sentence."

# 5. Open the interactive Roster shell and test a live slice or issue run
roster
# Inside roster>:
#   /status
#   /fleet list
#   /issues
#   /plan <your bounded task description>
#   (Press Enter to run planner -> coder -> reviewer in an isolated .worktrees/ worktree)
```

## NVIDIA OpenShell Integration & Onboarding (`rust/openshell.rs`)

Roster integrates **NVIDIA OpenShell (`v0.1.x`)** as an opt-in, deny-by-default
kernel-enforced sandbox, task-scoped YAML policy compiler, and zero-knowledge
inference provider router ([`rust/openshell.rs`](rust/openshell.rs)):

1. **Installation & Native Windows vs. WSL 2 Operation**:
   - `github-agent-roster` builds and runs **100% natively on Windows, Linux, and macOS** (`cargo build --release --locked --offline`) with **zero new Cargo dependencies** (no `openshell-sdk` or gRPC dependency bloat).
   - **WSL is not required** to install Roster, run `roster` / `roster shell`, run `roster onboard`, compile/verify OpenShell policies (`compile_task_policy`, `prove_openshell_policy_boundary`), or run `roster doctor --openshell`. On Windows, Roster uses native Win32 Job Objects (`process-wrap` + `win32job`) for host process-tree containment and Windows ACLs for `.roster/` and Vault files.
   - When executing live OpenShell sandboxes on a Windows host, OpenShell's gateway and sandbox workload run inside **WSL 2** (with Docker/Podman or MicroVM) because OpenShell's `filesystem_policy` relies on the **Linux kernel Landlock LSM (ABI v3+)** (`landlock.compatibility: "hard_requirement"`).
2. **Onboarding & `.roster/fleet.yml` Bridge**:
   - `roster onboard [--discover]` configures `.roster/config.yml` and `.roster/fleet.yml` as usual without requiring OpenShell to be installed.
   - `compile_fleet_provider_profile` translates any profile in `.roster/fleet.yml` into an OpenShell `category: inference` provider profile:
     - Host-local endpoints (`127.0.0.1`, `localhost`, `[::1]`) are mapped to `host.openshell.internal:<port>` so sandboxed workloads reach host vLLM/Ollama servers through the OpenShell supervisor proxy.
     - Remote/LAN endpoints (`aperture.xen.blackmesaresearch.ai:443`, `192.168.1.48:8000`) keep their exact host/port and binary allow-list (`binaries`).
     - `api_key_env` is emitted as `placeholder_only: true` (`OPENSHELL_SUPERVISOR_PLACEHOLDER` in `openshell_launch_environment`) with `OPENSHELL_TELEMETRY_ENABLED=false`; `GITHUB_APP_*` credentials are strictly rejected.
3. **Offline Readiness Diagnostics (`roster doctor --openshell` / `/doctor openshell`)**:
   ```powershell
   roster doctor --openshell --opt-in --runtime <PATH> --gateway http://127.0.0.1:8080 --policy <PATH> --provider rtx-3090-qwen
   ```
   Evaluates declared readiness offline and reports deterministic blocker codes (`OPENSHELL_NOT_OPTED_IN`, `OPENSHELL_RUNTIME_UNPINNED`, `OPENSHELL_GATEWAY_UNCONFIGURED`, `OPENSHELL_POLICY_MISSING`, `OPENSHELL_PROVIDER_UNBOUND`, `OPENSHELL_LANDLOCK_UNVERIFIED`, `OPENSHELL_LIVE_DISPATCH_UNAPPROVED`).

See [native runtime status](docs/RUST.md), [testing](docs/TESTING.md), and
[contracts dependency](docs/DEPENDENCY.md).

## Product and authority

The [feature spec](docs/FEATURE_SPEC.md), especially sections 3, 5, 7 and 8,
remains authoritative. Main/Git history supplies the old behavioral contracts;
native acceptance cases must preserve their expected results and errors.

- One writer per worktree; planner, coder and reviewer are sequential seats.
- No source comments beyond required legal notices.
- Minimal dependencies and measured performance, not assumed language gains.
- Unknown usage stays unknown; cancellation revokes late success.
- Lock age does not prove owner death; uncertain cleanup retains ownership.
- Humans own policy, workflows and release. No workflow or policy bypass.
- Publication uses the reviewed App SDK with complete credentials, never a
  human-authenticated commit, push or PR.

Node is still needed for the independent contracts publisher. Its presence
does not justify retaining a Node Roster library.
