# github-agent-roster

Roster is being rewritten as a native Rust software-delivery orchestrator.
GitHub Issues and PRs remain the board and forge. Planning, task files, seats,
tools, skills, memory and worktrees belong to Roster; identity and publication
belong to the independent [contracts SDK](vendor/github-agent-contracts).
There is no Kanban database, external worker framework or Node runtime fallback.

## Migration status

Ray authorized the full rewrite and explicitly directed this branch to remove
the Node implementation and oracle-preservation changes. The old application
and tests remain available in Git history, not as a parallel implementation or
frozen checkout here. Main now contains the merged Rust checkpoint.

The [Rust library](rust) implements configuration, fleet/capability catalogs,
lossless JSON/Text, recipes, request/provenance metadata, HTTP/SSE transport,
cancellation, state/locks/migration, planning, guarded execution, and actual
bounded builtin coder/read-only reviewer loops. The first local delivery
coordinator now connects those APIs to the CLI on the verified Windows backend.
The [native CLI](rust/main.rs) currently exposes:

- `roster --help`
- `roster init` — exclusive examples and a human-owned-policy note, offline.
- `roster doctor` — offline configuration, approved-image and contracts checks.
- `roster fleet list` — read-only configured profile metadata, without endpoint
  discovery, credential-file access, warming, default changes or automatic routing.
- `roster status --issue N [--offline] [--json]` — bounded native issue-log
  metadata, plus App-authenticated issue observations online. Unknown liveness,
  verification and PR state stay unknown.
- `roster stats [--json] [--ref REVISION_OR_RANGE] [--evals PATH]` — bounded,
  read-only local Git/run/evaluation aggregates.
- `roster recommend --task-class feat|fix|docs|test [--difficulty 1-5] [--json]`
  — advisory human-evidence ranking, constrained by the coder/model ceiling.
- `roster eval TARGET accept|reject|rework 1-5 y|n --local|--post [--minutes N] [--comment TEXT]`
  — an explicit human decision; optional posting verifies a human GitHub
  principal and one commit-associated PR. Private comments remain local.
- `roster run --seat coder --runtime builtin` — an existing local TASK.
- `roster run --ask-file PATH --runtime builtin --no-fetch` — a new isolated
  local task, including configured planning, coding and read-only review.
- `roster recipe validate PATH`
- `roster catalog parse ROOT FIELD...`
- `roster arguments split`

GitHub issue delivery/publication, resume, review repair, outer routing,
vault storage, onboarding and shell remain incomplete. Unsupported commands fail rather than invoke
Node. Removing old code or passing component tests is not a completed rewrite,
platform release or harness acceptance claim.

Analytics requires an absolute approved `ROSTER_GIT`, never Node or a metrics
exporter. Unknown measurements remain unknown. These commands do not record a
human evaluation, promote history into memory, contact GitHub, or change a
model/profile assignment. Explicit human evaluation is a separate path;
it refuses App/agent-declared invocations and never grants policy. `--local`
does not contact GitHub; `--post` additionally requires an approved absolute
`ROSTER_GH` and explicit human `GH_TOKEN` or `GITHUB_TOKEN`. It does not borrow
an operator authentication store or use the App publisher.
Automatic fleet/prior routing and `stats --delivery` remain unavailable.
See [local metrics](docs/METRICS.md)
for selectors, privacy boundaries and native format/platform limits.

Offline status requires an absolute approved `ROSTER_GIT`, but never contacts
GitHub, loads endpoint configuration or records history. It selects a registered
base issue worktree, ignores an unfinished trailing record and never emits raw
log details. Only a missing log permits canonical-repository fallback; corrupt,
linked or unreadable logs fail explicitly. This is local observation, not a
review or acceptance receipt. Without `--offline`, status additionally requires
complete App credentials, the clean reviewed contracts dependency and an
absolute approved `ROSTER_NODE20` for its identity adapter. Native Rust performs
the fixed GitHub HTTPS read and acknowledged token revocation; human tokens or
authentication stores are never substituted. This observes the issue only, not
its PR, acceptance or task execution. Automatic issue inference, retry selection
and the broader cached-issue/artifact display remain incomplete.

Fleet listing uses the same checked canonical private settings as native
delivery, including from linked worktrees. It preserves the tabular profile,
model, base URL, capacity, concurrency, hardware and task-hint display; the
onboard-default capacity sentinel remains `unknown`, not invented capacity.
Notes and key declarations are not displayed, and known credential values are
redacted. Other fleet commands and onboarding remain unavailable.
See [fleet scope](docs/FLEET.md#native-scope).

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

See [installation and host configuration](docs/INSTALL.md) and the exact
[local-delivery contract](docs/RUST.md#native-local-delivery-checkpoint).

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
