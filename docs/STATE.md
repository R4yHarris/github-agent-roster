---
title: State Ownership — Machine History, Machine Memory, Repository State, and Worktree State
status: normative
version: 1.0.0
spec: docs/FEATURE_SPEC.md
spec_sections: [3, 5.4, 5.6, 5.8, 7]
blocks: [#198, #195, #196, #197, #200, #201]
depends_on: [#194]
---

# State Ownership: Machine, Repository, and Worktree

This document is **normative for state ownership**, subject to `docs/FEATURE_SPEC.md`, which
remains the product intent. If this document conflicts with the feature spec, implementation
stops until a human resolves the conflict. This document defines *who owns each piece of
persisted state*, *where it lives*, *what may read it*, and *what may never happen to it*.

It exists so that runtime paths can be moved (issue #194 first slice) without inventing new
ownership rules on the fly, and so that no future change accidentally turns a local tool into
a cloud service, a second Kanban, or a source of fabricated metrics.

---

## 1. Normative principles

These restate `docs/FEATURE_SPEC.md` §3 and §7 and bind every decision below.

1. **Local-first.** All persisted state lives on the machine that produced it, or inside the
   repository it belongs to. Nothing is uploaded, replicated, or synced.
2. **Default-deny.** A datum is *not* available to a consumer unless an explicit rule in this
   document grants it. Silence means "no."
3. **One owner per artifact.** Every persisted artifact has exactly one owning scope
   (machine / repository / worktree / session) and exactly one writer. If two scopes appear to
   want a file, this document's ownership table decides.
4. **No silent overwrite or delete.** Migrations create new files; they never destroy old ones
   without an explicit, logged, user-visible action.
5. **Unknown metrics stay unknown.** The system never reports a number it did not measure. It
   does not extrapolate, average from stale data, or reuse a last-known value. If a metric is
   missing, the reported value is "unknown," not a guess (§7).
6. **No policy or workflow storage.** `.roster/` and machine state carry *state*, never
   policy. Workflow definitions, review gates, and scoring rules live in the repository as
   reviewed files, or not at all.

---

## 2. Stable terms

These names are binding. Runtime code, schemas, and docs must use them verbatim.

| Term | Definition |
| --- | --- |
| **Machine history** | Append-safe, per-user, per-repository-identity provenance of what actually happened: IDs, timestamps, outcomes, measured usage, tool outcome metadata, and references. It stores bounded redacted summaries, not unrestricted prompts, transcripts, source bodies, or tool output. Raw, unreviewed, never sent to a model. It is a record, not knowledge. |
| **Curated machine memory** | Explicitly promoted, human- or rule-approved extracts derived from machine history for a repository identity (conventions that worked, repeated failure causes, settled decisions). Curated by definition: a transcript becomes memory only via a promotion step. |
| **Repo state** | Durable, version-controlled-adjacent state scoped to one repository checkout, stored under `.roster/` in the working tree. Carries the record schema version and per-repo run records. |
| **Run record** | One unit of persistence describing a single agent run: inputs, identity, outcome, measured metrics, and pointers to artifacts. The atomic unit of machine history. |
| **Session** | The live, in-process lifetime of one agent invocation. Ephemeral. A session that ends without writing a run record leaves no trace by design. |
| **Issue/worktree state** | Ephemeral per-issue scratch state (task-local paths, temp files, unresolved-conflict markers) tied to one worktree or one issue worktree under `.worktrees/`. Safe to delete at any moment without losing knowledge. |
| **Published record** | The deliberate, redacted, minimal projection of a run record that leaves the machine — into a PR description, an export, or a review comment. Its contents are allow-listed in §7. |

The distinction that matters most: **machine history is raw; curated machine memory is
promoted.** No code path may treat all transcripts as memory. Absent an explicit promotion
step, machine history is unreadable by anything except diagnostics and cleanup.

---

## 3. Ownership scopes and precedence

There are exactly three persisted scopes. Each artifact belongs to exactly one. A live
session is not a fourth persisted scope: its process memory and OS-temporary files are part
of issue/worktree state and disappear when the process ends.

| Scope | Root | Backed up by git? | Lifetime |
| --- | --- | --- | --- |
| Machine | Platform-native per-user state directory (§4) | No | Until explicit history or memory cleanup |
| Repository | `<canonical-repo-root>/.roster/` | No (gitignored) | Until repo-state reset or checkout deletion |
| Issue/worktree | Managed files in the issue worktree plus its `.roster/` scratch area and Git-common artifact archive | No | Until issue cleanup or worktree removal |

Override precedence for choosing the machine root, from strongest to weakest:

1. **`ROSTER_STATE_HOME`** — explicit nonempty absolute-path override, primarily for tests
   and portable installs.
2. **Platform-native default** (§4). On Linux only, `XDG_STATE_HOME` participates in that
   default according to the XDG specification.

There is **no repository or current-directory fallback** for machine history or curated
machine memory. An unresolved, unsafe, or unwritable machine root fails that persistence
operation with an actionable error; a configured run may explicitly opt out of durable
history and continue without claiming it was recorded.

Repository state has one root: `<canonical-repo-root>/.roster/`. Linked worktrees resolve the
same canonical repository root through Git's common directory. Per-worktree artifacts remain
under the worktree that owns them and are never treated as shared repository state. A future
explicit test override may relocate repo state only inside the canonical repository root;
there is no environment or sibling-repository fallback.

Overrides never widen sensitivity: relocation grants no additional reader and must pass
canonicalization, containment, symlink/reparse-point, and permission checks.

---

## 4. Platform-native machine roots

Machine roots are per-user, never per-machine-shared and never per-system. Paths below use the
real conventions for each platform.

| Platform | Default root | Notes |
| --- | --- | --- |
| Windows | `%LOCALAPPDATA%\Roster\state\` | `ROSTER_STATE_HOME` is the only override. Missing `LOCALAPPDATA` fails closed. |
| macOS | `~/Library/Application Support/Roster/state/` | `ROSTER_STATE_HOME` is the only override. |
| Linux | `${XDG_STATE_HOME:-~/.local/state}/roster/` | `ROSTER_STATE_HOME` wins; `~/.config/roster` is not state. |

Sub-layout under the machine root:

```
<machine-root>/
  history/
    <repo-identity>/
      runs/<yyyy>/<mm>/<run-id>.json
      index.ndjson
  memory/
    <repo-identity>/
      curated/<memory-id>.json
      draft/<memory-id>.json
  locks/
    <repo-identity>.lock
  tmp/
```

Permissions: machine root and all children are user-only. On Windows this is the user profile
ACL; on macOS and Linux this is `0700` on directories and `0600` on files. A root that exists
with wider permissions is refused (§6, failure behavior), not silently tightened or silently
accepted.

Concrete examples:

- **Windows, normal clone:**
  `C:\Users\alice\AppData\Local\Roster\state\history\github.com--example--github-agent-roster\runs\2026\03\20260314T091200Z-a1b2.json`
- **macOS, linked worktree:** worktree state lives in the worktree; machine history still
  resolves to the main repo's identity:
  `/Users/alice/Library/Application Support/Roster/state/history/github.com--example--github-agent-roster/runs/2026/03/20260314T091200Z-a1b2.json`
- **Linux, normal clone:**
  `/home/alice/.local/state/roster/history/github.com--example--github-agent-roster/runs/2026/03/20260314T091200Z-a1b2.json`
- **Linux with `ROSTER_STATE_HOME=/mnt/roster-state`:**
  `/mnt/roster-state/history/github.com--example--github-agent-roster/runs/2026/03/20260314T091200Z-a1b2.json`

---

## 5. Current inventory and target ownership

The migration starts from the paths that exist today. “Target” is the owner after #198,
#196, and #195; this document does not pretend those migrations already exist.

| Current artifact | Target owner/root | Lifecycle and sensitivity | Writer | Reader | Cleanup |
| --- | --- | --- | --- | --- | --- |
| `.roster/config.yml`, `fleet.yml`, `capabilities.yml` | Repository `.roster/` | Durable private configuration; endpoint metadata, no secrets | onboard/settings/fleet | runtime, doctor, router | repo-state reset after preview |
| `.roster/bench.json` | Repository `.roster/` | Replaceable benchmark evidence | bench | operator/router only when typed | repo-state reset |
| `.roster/evals.jsonl` | Machine history keyed by repository identity | Durable human evaluation evidence | eval command | typed metrics/routing API | machine-history prune |
| `.roster/memory/{planner,coder}.jsonl` | Curated machine memory keyed by repository identity and seat | Durable context; redacted summaries only | seat memory API | matching seat through typed API | curated-memory reset |
| `.roster/runs/runs.jsonl` | Machine history | Append-safe run provenance; real/unknown usage | run recorder | typed metrics/publication API | machine-history prune |
| `.roster/runs/*.log`, `.roster/logs/debug-*.jsonl` | Machine history diagnostic area | Bounded, redacted operational traces | run/debug logger | operator diagnostics | trace retention/prune |
| `.roster/history` | Machine-local shell history | Bounded safe commands, never vault commands | interactive shell | same user shell | shell-history clear |
| `.roster/checkpoints/<task>/`, `.roster/map.md` | Issue/worktree `.roster/` | Ephemeral resume/map material | checkpoint/map writer | active issue run | issue cleanup |
| `.roster/asks/` and local draft assignments | Issue/worktree state | Ephemeral intake/handoff | ask/prepare | matching run | issue cleanup |
| `.worktrees/<task>/` | Issue/worktree state | Source checkout and managed `ASSIGNMENT`, `TASK`, `RECIPE`, `ESTIMATE`, `RESULT`, `REVIEW`, `PLAN`, `CONTEXT` artifacts | orchestrator and seats | matching seats/human | issue cleanup / `git worktree remove` |
| Git common dir `roster-artifacts/<task>/` | Issue/worktree state | Archived generated handoffs for resume/debug | run-artifact archiver | matching run/operator | issue cleanup |
| `~/.roster/vault/` (legacy/current) | Machine vault, separate from history and memory | Credentials; highest sensitivity | vault API only | secret resolver only | explicit vault delete, never state cleanup |
| `.env` copied into an issue worktree | Issue/worktree state | Credential-bearing compatibility input; must not enter records | worktree preparation | endpoint process only | issue cleanup |
| Git issues, branches, PRs, trailers, checks, comments | Published record on GitHub | Authoritative delivery evidence | App/human policy paths | GitHub participants | GitHub lifecycle, never local cleanup |

Every target artifact carries or is governed by one schema version and one cleanup surface.
No run/eval/history copy remains in repo state after migration; repository state contains only
configuration and operational indexes/checkpoints needed by that repository.

### 5.1 Repository scope

`<canonical-repo-root>/.roster/` is shared logically by linked worktrees through the state
resolver, not by assuming the current worktree path. It contains private configuration,
fleet/capability declarations, schema and migration markers, active-run indexes, and
repository-specific operational locks. It contains no vault secret and no durable machine
history. Runtime paths must be gitignored before first write and must never be staged.

### 5.2 Issue/worktree scope

The issue worktree owns its managed handoff/result files, checkpoints, map, scratch files,
and copied `.env`. The Git common directory may hold an archive for that issue because it
survives replacement of generated handoffs, but it remains issue-scoped and cleanup-safe.
An active-run lock prevents cleanup while a writer owns the worktree. Removing one issue's
state must not touch another issue, shared repo state, machine history/memory, the vault, or
published GitHub evidence.

---

## 6. Repository identity

Machine state must be resolvable to the *same repository* across path changes, worktrees,
reclones, and offline work. The identity key is **`<host>--<owner>--<name>`**, lowercased,
computed as follows:

1. **Resolve the repository root.** From the current working directory, walk upward to find a
   directory containing `.git`. If `.git` is a file (linked worktree), read its `gitdir:`
   pointer and resolve the main repo root from it.
2. **Resolve the remote, if any.** Read `origin`'s URL. Accept `https://`, `ssh://`,
   `git@host:owner/name.git`, and local paths. Normalize: lowercase host, strip `.git`,
   strip `www.`, map `ssh://git@host/owner/name` and `git@host:owner/name` to
   `host/owner/name`. If `origin` is absent, fall back to the first configured remote in
   `git config --get-remote-list` order; if that yields nothing, go to step 3.
3. **Offline repo / no remote.** Derive `local--<root-commit-oid>` when the repository has a
   root commit. For an unborn repository, generate a cryptographically random repository ID
   once and store it in repo state; until that ID exists, durable machine persistence is
   refused. Offline identity is flagged `"identity_strength": "local"`. Basenames and
   absolute checkout paths are never identity inputs because they collide or break on moves.
   Identity never depends on the checkout directory, so a reclone or replaced checkout of
   the same remote keeps reading prior machine history. `ROSTER_REPO_ID`, when set,
   overrides the derived id (`resolveRepositoryIdentity` in `src/lib/paths.mjs`) for
   checkouts without a usable remote.
4. **Forks are distinct repositories.** Identity includes the owner, so
   `owner/name` and `other-fork/name` never share machine history or memory. There is no
   fork-merging rule; if a fork's memory is wanted upstream, memory entries must be
   re-promoted there explicitly.
5. **Worktrees share identity.** A linked worktree resolves to its main repository's
   identity, so both worktrees write machine history under the same key.
6. **Reclones keep identity.** Identity does not depend on path, so deleting and recloning
   preserves history.
7. **Rename or remote change creates a new identity.** If `origin` URL changes in a way that
   changes `<host>--<owner>--<name>`, the old history is not merged, renamed, or deleted. It
   remains at the old key, is excluded from active lookups, and is eligible for the retention
   policy in §8. The change is surfaced as a diagnostic on the next run.

### 6.1 Worked examples

| Scenario | Resolved identity | Notes |
| --- | --- | --- |
| Repo A: `https://github.com/example/github-agent-roster.git` at `D:\oss\github-agent-roster` | `github.com--example--github-agent-roster` | Remote-derived identity. |
| Repo A, linked worktree `D:\oss\github-agent-roster\.worktrees\issue-199` | Same as Repo A | Resolved through Git common-dir metadata. |
| Repo A, recloned at `D:\oss\github-agent-roster-v2` | Same as Repo A | Identity is remote-derived, not path-derived. |
| Repo A with all remotes deleted | `local--<root-commit-oid>` (`"identity_strength": "local"`) | Stable across moves and clones retaining history. |
| Repo B: `git@github.com:example/other-repo.git` | Different identity | Never shares memory with Repo A. |
| Repo A on a shared machine, user `bob` | Same identity, but under `/home/bob/...` | Identity is per-user-rooted; see §10. |

### 6.2 Table-driven scenarios (required coverage)

| # | Scenario | Main repo path | Worktree path | Machine history root | Repo state root | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Repo A, normal clone (Linux) | `~/code/roster` | — | `~/.local/state/roster/history/github.com--example--github-agent-roster/` | `~/code/roster/.roster/` | Baseline. |
| 2 | Repo A, linked worktree | `~/code/roster` | `~/code/roster/.worktrees/issue-199` | Same key as #1 | canonical repo `.roster/`; issue artifacts stay in linked worktree | Shared repo state and issue state remain distinct. |
| 3 | Repo B, second repo | `~/code/other` | — | `~/.local/state/roster/history/github.com--example--other-repo/` | `~/code/other/.roster/` | Separate key; no cross-reads. |
| 4 | Reclone of Repo A | `~/code/roster-recloned` | — | Same key as #1 | `~/code/roster-recloned/.roster/` | Old repo state is gone; machine history survives. |
| 5 | Offline repo with no remote | `~/code/offline` | — | `~/.local/state/roster/history/local--<root-commit-oid>/` | `~/code/offline/.roster/` | Local-strength identity; unborn repos need a generated ID before persistence. |
| 6 | Shared machine, two users | `/home/alice/code/roster`, `/home/bob/code/roster` | — | `/home/alice/.local/state/roster/history/<key>/` and `/home/bob/.local/state/roster/history/<key>/` | each user's own checkout | Identity string matches; roots do not. No sharing occurs. |

---

## 7. Data flow: what may go where

Default-deny. A cell is "no" unless the row's rule grants it.

| Artifact | Model context | Routing/eval calc | Diagnostics | Exports | PR metadata |
| --- | --- | --- | --- | --- | --- |
| Machine history (raw run records) | **No** | Aggregate counts only, never raw records | Yes | Only via explicit user action, redacted | **No** |
| Curated machine memory | Yes, when repo identity matches | Yes, via same rules as model context | Yes | Redacted subset only, user-initiated | Only as an explicitly cited decision, never verbatim logs |
| Repo state (`.roster/state/repo.json`, recent runs) | No | Yes, for this repo only | Yes | Redacted summary only | Only schema version + outcome, never paths or metrics |
| Run record | No | Yes, for the same repo identity | Yes | Redacted subset | Only published record fields |
| Session (live) | Yes, ephemeral | Yes | Yes | No | No |
| Issue/worktree state | Yes, for the active task only | No | Yes | No | No |
| Published record | Yes | Yes | Yes | Yes | Yes, by definition |

**Published record allow-list.** A published record may contain: outcome, repo identity,
schema version, which tools succeeded or failed, and any *measured* metric the user opted
into. It may not contain: machine paths, usernames, environment variables, un-redacted
diffs, other repositories' data, or any secret.

**Unknown metrics stay unknown.** If a metric was not measured in this run, the field is
emitted as `"unknown"` or omitted entirely — never `0`, never a carried-forward value, never
an average. Downstream routing and eval code must treat `"unknown"` as a distinct value, not
as falsy-zero.

---

## 8. Record schemas, migration, retention, and integrity

### 8.1 Versioned schemas

Every persisted record carries `schema_version` as a positive integer at the top level.
Schema rules:

- Adding a field with a safe default: **minor**, allowed in place.
- Removing, renaming, or changing the meaning of a field: **major**, requires a migration
  step.
- Writers always write the *current* version. Readers accept the current version and the
  version immediately before it.

### 8.2 Forward migration

On read, if `schema_version` is older than current:

1. Copy the file to `<file>.pre-migrate` alongside the original. Never modify in place first.
2. Apply the migration in memory, producing the current version.
3. Write the migrated content to the original path using the atomic-write procedure in §8.3.
4. Write a receipt to `<machine-root>/history/<repo-identity>/migrations.ndjson` recording
   path, from-version, to-version, and timestamp.
5. Keep `.pre-migrate` files. Retention policy (§8.5) may prune them later; code must not.

#### Legacy layout to split layout

`migrate(root)` in `src/lib/repo-migrate.mjs` moves the pre-split `.roster` layout toward
the split `.roster-state` layout. It is **copy-only**. It copies only the declared
per-worktree state subtrees (`checkpoints/`, `runs/`) into
`.roster-state/worktrees/<key>/` and never deletes, renames, or rewrites a legacy file.
Private config, fleet and capability declarations, memory, history, and locks stay in
`.roster` (default-deny), so no credential-bearing file moves.

- **Dry-run.** `{ dryRun: true }` reports counts, layout names, active-run state, and the
  retained entries without writing anything.
- **Active runs.** A live lock under `locks/` refuses the migration before any write.
- **Backup and record.** The migration writes a backup manifest and an in-progress record
  under `.roster-state/migration/` before its first copy.
- **Rollback.** A failed copy rolls back every file the migration created or refreshed.
- **Interrupted runs.** The next run detects an interrupted migration and completes it.
- **Errors.** A corrupt record or a destination edited by someone else fails closed, with
  recovery steps.
- **Reruns.** A second run plans zero changes. Legacy state that grew since the last run
  refreshes only the copies the migration itself wrote.

### 8.3 Locking and atomic writes

- **Locks.** Every writer takes `<machine-root>/locks/<repo-identity>.lock` before touching
  machine-scoped paths for that identity, and a lock inside `.roster/state/` before touching
  repo-scoped paths. Locks are advisory but mandatory for all in-tree writers.
- **Atomic writes.** Write to `<target>.tmp-<pid>-<nanos>` in the same directory, `fsync`,
  then `rename` over the target. Never write in place, never write across filesystems.
- **Append-only files** (`index.ndjson`, `migrations.ndjson`) are appended under lock with a
  single `O_APPEND` write per line.

### 8.4 Corruption handling

- A record that fails to parse is moved to `<file>.corrupt-<timestamp>`, a diagnostic is
  surfaced, and the run continues as though the record did not exist.
- An append-only index with a truncated final line drops the incomplete line and records the
  drop.
- **Unknown metrics stay unknown after corruption.** A corrupt run record must not cause a
  downstream metric to be reported as zero or as a stale value; it must be reported as
  unknown.
- Never silently delete a corrupt file.

### 8.5 Retention

| Artifact | Retention | Cleanup |
| --- | --- | --- |
| Machine history run records | Default 90 days, configurable upward | `roster state prune-history --older-than 90d` |
| `.pre-migrate` files | Pruned only by explicit user action | `roster state prune-migrations` |
| `.corrupt-*` files | Until user clears them | `roster state clear-corrupt` |
| Curated memory | Until explicitly unpromoted | `roster memory unpromote <id>` |
| Repo state recent runs | Last 50 per repo | `roster state prune-repo-runs` |
| Issue/worktree state | Deleted when worktree pruned | `git worktree remove` + `roster state prune-worktree <id>` |
| Session | Gone at process exit | N/A |

`RETENTION_POLICIES` in `src/lib/paths.mjs` declares one policy per scope (`machine`, `repo`,
`worktree`, `run`) with a distinct root and default window (machine history: 90 days).
`resolveRetentionConfig(env)` in `src/lib/config.mjs` reads `ROSTER_RETENTION_OPT_OUT`
(`true`/`false`) and positive-integer `ROSTER_RETENTION_WINDOW_MS_<SCOPE>` overrides, rejecting
anything else. `evaluateRetention(policy, { nowMs, windowMs, optOut, createdAtMs })` in
`src/lib/repo-state.mjs` is pure over an injected clock; it keeps everything when opted out
and throws on a missing or invalid record age rather than treating it as expired.

`roster clean --target <target>` is the shipped cleanup surface (#278). It is a dry-run preview
unless `--execute --yes` is given, and each target resolves only its own root:

| Target | Deletes | Never touches |
| --- | --- | --- |
| `issue --issue N` | Machine state for `.worktrees/issue-N` (`repos/<id>/worktrees/wt-*`) | Other issues, repo state, the checkout, `.git` |
| `repo` | Shared repo state under `repos/<id>/` | `identity`, `schema.json`, `worktrees/`, `locks/`, machine history, other repos |
| `machine-history [--store DIR]` | Raw-history and compaction provenance records | Curated memory, malformed records (use repair) |
| `curated-memory [--store DIR]` | Curated-memory provenance records | Raw provenance |

Provenance pruning holds the store lock, reports only record ids, and rebuilds the segment
index so pruned content does not persist. GitHub issues, PRs, and their comments are never
touched and remain the durable evidence after any cleanup.

### 8.6 Secret redaction before persistence

Redaction happens **before** any write, not as a post-hoc filter.

- Known secret-shaped values (tokens, keys, credentials, `Authorization` headers, private
  key blocks) are replaced with `[REDACTED:<type>]` at the boundary where data enters a
  run record.
- Environment variables are never persisted verbatim, only their *names* as a set.
- Redaction is applied to every field of a record, including nested and future fields; it is
  not an allow-list-dependent step.
- A redaction failure is a hard error: the record is not written.
- Verification of this behavior in tests uses obvious non-credential sentinels such as
  `test-only-private-api-key`, and asserts they are absent from the code's output. Never use
  real or realistic-looking credentials in fixtures.

### 8.7 No silent overwrite or delete

- Migration creates `.pre-migrate`; it never overwrites without a backup.
- Opening an existing install never deletes unknown files under `.roster/` or the machine
  root. Unknown files are left alone and reported as unrecognized.
- Deleting a repo directory does not delete machine history. Machine history is keyed by
  identity and outlives the checkout.

---

## 9. Threat model

| Threat | Vector | Mitigation |
| --- | --- | --- |
| Cross-repo leakage | Repo B's memory or history read while working in Repo A | All reads keyed by resolved repo identity; identity is resolved from the working repo, not from any cached value; curated memory promotion is repo-scoped; exports require explicit user action |
| Symlink / path traversal | A symlink inside `.roster/` or `<machine-root>` pointing outside, or a run id like `../../etc/passwd` | Reject symlinked entries inside state roots; canonicalize and verify every resolved path is inside its declared root; ids are validated against `^[A-Za-z0-9][A-Za-z0-9._-]*$`; refuse `..`, absolute, and multi-segment ids |
| Shared machine | Another local user reads or tampers with state | Machine root is user-only `0700`/`0600`; a root with wider permissions is refused, not adopted; no machine-wide or group-writable state root is ever created |
| Backups | Machine or repo state captured into an unintended backup target | Redaction happens before persistence, so nothing secret is ever present to back up; published record is the only intended egress and is allow-listed; `.roster/` is gitignored so it does not ride along in repo archives |
| Accidental git inclusion | `.roster/` or worktree state committed and pushed | `.gitignore` lists every runtime state path under `.roster/` explicitly plus `.roster-state/`, with no blanket `.roster/` rule, so tracked templates, docs, and any shared setup a repo chooses to commit stay trackable; writers verify it is ignored and refuse to write if not; state writers never run `git add`; `scripts/check-roster-scope.mjs` (`checkRosterScope`) fails a commit scope check when a runtime state path is missing from `.gitignore` or present in the index |
| Identity collision | Two different repos resolving to the same key under a weak identity | Weak identity is flagged; consumers must treat weak-identity records as non-authoritative; collisions are reported, not silently merged |
| Memory poisoning | A model-adjacent transcript quietly becomes "memory" | Memory requires an explicit promotion step; no code path may read machine history as context |

---

## 10. Failure behavior

- **Machine root missing, unsafe, or unwritable** → fail the persistence operation with an
  actionable diagnostic. Never fall back to the repository or current directory. A run may
  continue only when durable history was explicitly disabled, and must not claim a record.
- **Machine root exists with wrong permissions** → refuse to use it, surface a diagnostic
  naming the path and observed permissions. Do not silently tighten or silently proceed.
- **Lock held** → wait with a bounded timeout, then fail the write with a clear error naming
  the lock path. Never break or steal a lock.
- **Identity unresolvable** (no repo root found) → operate in session-only mode. Persist
  nothing except ephemeral session state. Do not guess an identity.
- **Fresh clone / replaced checkout** → a missing state directory is created on first open
  with an empty state object (`openRepoState` returns `initialized: true` and `state: {}`).
  Reads return `null` rather than throwing, so empty is indistinguishable from absent and
  the empty-state contract holds. Runtime state files carry no secrets, and deleting
  `.roster/` never touches vault secrets, tracked source, machine history, or published
  GitHub evidence.
- **Redaction error** → do not persist the record. Surface the error.

---

## 11. Explicitly out of scope, permanently

The following are rejected by this design. Reintroducing any of them requires changing this
document first.

- **A Kanban DB.** `.roster/` is not a project tracker. No board, no card, no column, no
  issue-status store. The repository's own issue tracker is the source of truth (§7).
- **Cloud sync.** No sync targets, no replication, no "restore from cloud," no telemetry
  upload, no anonymous crash reporting that leaves the machine.
- **Policy or workflow storage.** `.roster/` never stores review gates, scoring rules,
  allowed-command lists, or workflow definitions. Those are reviewed code in the repository.
  Storing them as state would let them change without review.
- **Machine-readable aggregate metrics with fabricated values.** Any metric not actually
  measured is `"unknown"`. This is a design invariant, not a TODO.
- **Cross-repo or cross-user shared state.** No shared cache, no team memory, no "organizational"
  layer. State is per-user and per-repo-identity, full stop.

---

## 12. Summary of ownership

| Artifact | Scope | Sensitivity | Writer | Reader | Cleanup |
| --- | --- | --- | --- | --- | --- |
| Machine history records, indexes, traces, evals, and safe shell history | Machine | Medium; redacted summaries and provenance | Typed history/run/debug/eval APIs under lock | Typed diagnostics, routing/eval, export APIs | `roster state prune-history` / scoped history clear |
| Curated memory and drafts | Machine | Medium; explicitly promoted context | Memory promotion and seat-memory APIs | Matching repository/seat context and diagnostics | `roster memory unpromote <id>` / curated-memory reset |
| Vault entries | Machine vault (separate) | Highest; credentials | Vault API | Secret resolver | Explicit vault deletion only |
| Private config, fleet/capabilities, schema/migration markers, active indexes/locks | Repository | Low–Medium; no credentials | Repo-state API | Runtime, router, doctor | Previewed repo-state reset |
| Managed handoffs/results, checkpoints, map, scratch, archive, copied `.env` | Issue/worktree | Low–High depending on `.env`; never history input | Orchestrator and matching seats | Active issue and operator | Previewed issue cleanup / worktree removal |
| Live process/temporary state | Issue/worktree (ephemeral) | Low–Medium | Active process | Active process | Process exit |
| Published issue/branch/PR/trailer/check/comment record | GitHub published record | Redacted, reviewed evidence | App or human-owned policy path | GitHub participants | GitHub lifecycle only |

Every artifact above maps to exactly one scope. Anything found persisted that is not in this
table is, by definition, a bug.
