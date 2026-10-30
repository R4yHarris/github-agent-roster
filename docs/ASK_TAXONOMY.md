# Software Engineering Ask Taxonomy & Orchestration Decision Graph

This document defines the canonical taxonomy of software engineering asks handled by `github-agent-roster`, the deterministic decision graph that routes each ask from intake to verified delivery, and the architectural safeguards that guarantee first-try SDLC completion under [FEATURE_SPEC.md](FEATURE_SPEC.md) sections 3, 4, 5.1–5.8, and 7.

---

## 1. End-to-End Ask Decision Graph

Every human prompt, CLI invocation, voice transcription, clipboard attachment, or GitHub Issue entering `roster` traverses a deterministic 5-stage decision tree into one of **9 Canonical Software Engineering Ask Archetypes**:

```text
User Prompt / GitHub Issue / Voice / Clipboard / Dragged Path (`roster> ` or `roster <cmd>`)
 │
 ├──► [Stage 0: Input Sanitization & Multi-Modal Attachment Resolution (`rust/attachments.rs`, `rust/shell.rs`)]
 │     ├─ Strip leaked terminal DSR/CPR (`\x1b[...R`), SGR mouse reports, and stray control characters (`\x00`..`\x1f`)
 │     ├─ Detect dragged/pasted file & directory paths (`detect_file_drop`) -> register `#1, #2, ...` (`AttachmentKind::{Image,Text,Binary,Directory}`)
 │     ├─ Expand `[Pasted text #N]` tokens and inline `@file`, `@file:start-end`, and `@dir/` workspace mentions
 │     └─ Transcribe `/voice` audio via local `whisper-cli` or OpenAI-compatible `/v1/audio/transcriptions`
 │
 ├──► [Stage 1: Direct Operator & Shell Dispatch (`rust/shell.rs:dispatch_line`)]
 │     ├─► `!command` ──► **[Archetype 1: Direct Host Shell Escape]** (`execute_bang_shell_command`, redacted transcript)
 │     └─► `/cmd` or NL command alias ──► **[Archetype 2: Operator & Fleet Control]**
 │           ├─ `/fleet [list|probe|use]`, `/route [on|off|auto|status]`, `/config`, `/vault`
 │           ├─ `/doctor`, `/doctor openshell --opt-in ...`, `/status`, `/usage`, `/context`, `/compact`
 │           ├─ `/paste`, `/copy [code|N]`, `/export`, `/checkpoints`, `/rewind`, `/undo`
 │           └─ `/board [N|list N|add N|move N|run N]`
 │
 └──► [Stage 2: Software Engineering Ask Classification (`rust/planning/intake.rs`, `rust/shell.rs`)]
       │
       ├──► **[Archetype 3: Protected-Surface & Secret Default-Deny Refusal]** (FEATURE_SPEC §3, §5.1, §5.4)
       │     ├─ Targets: `agent-policy.yml`, `.github/workflows/*`, `.env*`, `*.pem`, `*.key`, `vendor/github-agent-contracts/**`
       │     └─ Outcome: Immediate deterministic refusal (`ROSTER_TOOL_DENIED`) before any mutation or commit
       │
       ├──► **[Archetype 4: Read-Only Workspace, Multi-Repo & Codebase Question]** (FEATURE_SPEC §5.3, §5.7)
       │     ├─ Triggers: `question` specKind, drive/repository inventory queries, or read-only file/architecture questions
       │     ├─ Path A (Scoped Question): `answer_repository_question` (`rust/ask.rs`) — bounded 1–8 file read, zero tools/writes
       │     └─ Path B (Workspace / Multi-Repo Inspection): `run_agentic_nlp_turn` (`list_dir`, `read_file`, `search_text`, `git_inspect`)
       │
       ├──► **[Archetype 5: Compliance, Security & SDLC Audit Report Generation]** (FEATURE_SPEC §5.4, §5.5, §5.7)
       │     ├─ Triggers: Requests to audit a repository against quality/security/SDLC standards and save a Markdown report
       │     ├─ Execution: Deep repository inspection (`deep_inspect_repositories`) + `read_file`/`search_text` + `write_report`
       │     └─ Outcome: Evidence-grounded Markdown report written to the requested path with secret redaction
       │
       ├──► **[Archetype 6: Directory-Scoped Local Ask]** (FEATURE_SPEC §5.3, §5.4, §5.5, §5.7; [REPL.md](REPL.md))
       │     ├─ Triggers: Prompt grants one relative output directory (`Output it in a new "<dir>" directory`, `reqs.output_directory.is_some()`)
       │     ├─ Execution: 3-Seat Isolated Worktree Pipeline (`planner -> coder -> reviewer`) in `.worktrees/local-<id>/<dir>/`
       │     │   ├─ Document intent (`directory_document`): Planner chooses `.md` files beneath `<dir>/`; skips `node --test`; Reviewer runs `Original Ask audit`
       │     │   └─ Executable intent (`requests_executable`): Planner chooses code + test files beneath `<dir>/`; Coder runs tests + counterfactual red/green
       │     └─ Outcome: Verified files in `.worktrees/local-<id>/<dir>/` alongside `TASK.md`, `RESULT.md`, and `REVIEW.md`
       │
       ├──► **[Archetype 7: Bounded Single-Slice Code, Bugfix, Refactor, or Multi-File Docs Task]** (FEATURE_SPEC §5.3, §5.4, §5.5)
       │     ├─ Entrypoints: `/run N`, `roster run --issue N`, `/ask`, `roster run --ask-file`, or NLP `start_sdlc_delivery`
       │     ├─ Intake Normalization:
       │     │   ├─ Unstructured GitHub Issues without `## Allowed Files` auto-infer target files from prose + repo index
       │     │   ├─ JS/ESM code slices without a test file auto-attach `tests/issue_N.test.mjs`
       │     │   └─ Inject ESM counterfactual import guidance (`import * as mod` for new exports; `await import(...)` for new modules)
       │     ├─ Seat 1 — Planner (`rust/planning/lifecycle.rs`, `grounding.rs`, `critic.rs`):
       │     │   ├─ Indexes symbols across tracked code AND config/IaC/markup files (`.json`, `.bicep`, `.yml`, `.toml`, `.html`, `.css`, `.py`, `.sh`, `.ps1`)
       │     │   ├─ Recognizes standard JS/Node/DOM/Rust runtime globals & members (`assert.*`, `Object.*`, `JSON.*`, `Array.*`, `Number.*`, `process.*`, etc.)
       │     │   └─ Synthesizes `TASK.md` (with `## Design` and `## New files`), `RECIPE.yml`, and `ESTIMATE.md`
       │     ├─ Seat 1.5 — OpenShell Boundary Prover (`rust/openshell.rs`, when `openshell.enabled: true`):
       │     │   └─ Compiles task-scoped `.roster/openshell/policy.yml` (Landlock `hard_requirement`) & verifies via `openshell-prover` + `profile lint`
       │     ├─ Seat 2 — Coder (`rust/seats/coder.rs`, `rust/execution/`):
       │     │   ├─ Dynamic exploration budget: `((files_allowed.len() + cited_task_files) * 2).max(8)` turns before first write
       │     │   ├─ Read scope permits all `files_allowed`, `required_reads`, imported modules, and files/directories cited in `TASK.md`
       │     │   ├─ Tool ergonomics: `read_file` accepts `offset` or `start_line`; `edit_file` accepts `old_string`/`new_string` or `old_str`/`new_str`
       │     │   ├─ Automatic gates: `verify_final` (`node --test` / `cargo test`), `shadow::check`, `counterfactual::run` (with 1 bounded `red-green` repair on base setup error), and `self_review` (with fresh test output & state reset on correction)
       │     │   └─ Emits sealed `RESULT.md`
       │     ├─ Seat 3 — Reviewer (`rust/seats/reviewer.rs`):
       │     │   ├─ Runs tool-free `Original Ask audit` over complete delivered files + referenced tracked files + diff
       │     │   ├─ Runs per-check verification (`Mode: verified`, `Verdict: pass`) and writes `REVIEW.md`
       │     └─ Post-Verify Delivery:
       │         ├─ Local issue/SDLC run: applies verified worktree files to repo, commits, closes issue, moves Kanban card to `Done`
       │         └─ App PR handoff (`--publish`): publishes via `vendor/github-agent-contracts/scripts/agent-pr.mjs`
       │
       ├──► **[Archetype 8: Greenfield End-to-End Application + IaC + Test Suite + Kanban Delivery]** (FEATURE_SPEC §5.3, §5.4, §5.7)
       │     ├─ Triggers: High-level product goal in a new or target repository with optional `GitHub Project #N` tracking
       │     ├─ Execution: `run_agentic_nlp_turn` (48-turn budget, 16-tool context retention) + **4-Stage Completion Gate** scoped to `effective_repo_dir`:
       │     │   ├─ Gate 1 (`missing_deliverables`): Prevents premature stop while any requested non-protected file is missing on disk
       │     │   ├─ Gate 2 (`auto:node_build`): Executes `node src/cli.mjs` when generator CLI exists and `dist/` is unbuilt; feeds errors back for repair
       │     │   ├─ Gate 3 (`auto:node_test`): Executes `node --test` across `tests/` and `test/`; feeds TAP failures back into loop until `exit_code == 0`
       │     │   └─ Gate 4 (Auto Commit/Push & Kanban Closeout): Commits & pushes verified artifacts and moves completed cards on `Project #N` to `Done`
       │     └─ Outcome: Complete multi-file application, passing test suite (`exit_code == 0`), `HUMAN_ACTION_CHECKLIST.md`, and `Done` Kanban board in 1 pass
       │
       └──► **[Archetype 9: Multi-Wave Feature / Initiative Breakdown & Kanban Board Execution]** (FEATURE_SPEC §5.3, §5.4, §5.7)
             ├─ Feature/Initiative Planning: Planner writes `PLAN.md` with wave-labelled child issue drafts (`wave:1`, `wave:2`, ...)
             └─ Board Execution (`/board run N`): Iterates through `Todo`/`Ready`/`In Progress` cards on `Project #N`, running Archetype 7 per card and moving each verified card to `Done`
```

---

## 2. Root-Cause Failure Taxonomy & Permanent Architectural Safeguards

The table below catalogs the systemic failure modes identified across multi-round live testing of `github-agent-roster` and the permanent architectural fixes implemented in the Rust library:

| # | Lifecycle Stage | Failure Symptom Observed in Earlier Runs | Root Cause in Library | Architectural Safeguard Implemented |
|---|---|---|---|---|
| **1** | **Intake (`intake.rs`, `shell.rs`)** | Unstructured natural-language GitHub issues failed intake with `Ask must name or declare allowed files`; `.svg`, `.xml`, and `.bicep` files in prose were ignored. | `execute_issue_delivery` did not infer `## Allowed Files` when `!reqs.explicit`, and `intake::requirements` omitted `svg\|xml\|bicep\|cjs\|jsx`. | Auto-infer `## Allowed Files` and `## Acceptance checks` for unstructured issues in `execute_issue_delivery` and include `svg\|xml\|bicep\|cjs\|jsx` in `intake::requirements`. |
| **2** | **Directory Routing & Validation (`shell.rs`, `task.rs`, `lifecycle.rs`)** | Pasting a directory-scoped Ask (`Output it in a new "new-design" directory`) at `roster> ` without `/ask` bypassed the 3-seat worktree pipeline, and `validate_directory_documents` rejected setup-guide checks mentioning `serving`/`hosting`/`created`/`live`. | `dispatch_line` checked `!reqs.explicit` without checking `reqs.output_directory.is_none()`, and `validate_directory_documents` matched normal English words (`created`, `live`, `and deploy`) as command-execution assertions. | Guarded `run_agentic_nlp_turn` with `reqs.output_directory.is_none()`, updated `slice_instructions` for `directory_document`, and scoped `validate_directory_documents` to require document-content verbs while rejecting only CLI command + execution-success assertions. |
| **3** | **Planner Grounding (`grounding.rs`)** | Planner checks citing `assert.strictEqual`, `Object.keys`, `JSON.parse`, or keys from `.json`/`.bicep`/`.html` files failed `grounding::errors` as "imaginary exports". | `runtime_member` only listed `fs`, `path`, `os`, `child_process`, and `read_context` only indexed `.js`/`.ts`/`.rs` files. | Expanded `runtime_global` and `runtime_member` to cover standard JS/Node/DOM/Rust builtins and indexed tokens from tracked `.json`, `.bicep`, `.yml`, `.toml`, `.html`, `.css`, `.py`, `.sh`, `.ps1` files in `read_context`. |
| **4** | **Coder Exploration (`coder.rs`)** | Multi-file documentation/integration tasks citing 6+ existing files failed on Turn 8 with `Exploration budget expired without an authorized named product change`. | `exploration` was fixed at `(files_allowed.len() * 2).max(8)`, ignoring how many existing files `TASK.md` required the coder to read before writing. | Scaled `exploration` to `((files_allowed.len() + cited_files) * 2).max(8).min(turns)` so tasks citing multiple existing files have sufficient read turns before their first write. |
| **5** | **Coder Read/Edit/Write Tools (`tools.rs`)** | Coder failed with `The same invalid tool call repeated without a successful correction` when reading TASK-cited files, passing `start_line` to `read_file`, passing `old_str` to `edit_file`, or rewriting a verbose `README.md`. | `Boundary::readable` blocked TASK-cited subdirectories/files under `readme_only`, `read_file_value` rejected `start_line`, `mutate` rejected `old_str`/`new_str`, and `write_file` blocked `<60%` length rewrites on `.md` files. | Allowed TASK-cited files and their parent directories in `readable`, accepted `start_line` and `old_str`/`new_str` aliases, and exempted `.md` files from the shorter-rewrite refusal. |
| **6** | **Counterfactual Red/Green (`counterfactual.rs`, `coder.rs`, `lifecycle.rs`)** | Adding a new ESM export or new module failed counterfactual evaluation with `status: unavailable` (`compiler/setup failure or missing named output is not a red test`). | Static `import { newFn }` or top-level import of a new module fails at ESM link time at `base` before `test(...)` executes, and `coder.rs` aborted immediately on `Status::Unavailable`. | Added ESM namespace-import (`import * as mod`) and dynamic `await import(...)` guidance to `slice_instructions` and `coder.rs`, supported multi-test `target_red_count` in `counterfactual::run`, and granted 1 bounded `red-green` repair in `coder.rs` when `redGreen` is `unavailable`. |
| **7** | **Shadow Module Gate (`shadow.rs`)** | Single-module library tasks adding a helper export or struct failed `shadow::check` with `no product module uses it (only tests, or nothing)`. | `shadow::check` required another product module to call every new export not verbatim in `TASK.md`, even when `files_allowed` only contained 1 product file. | Exempted tasks with `<= 1` allowed product file (and entry-point files `lib.rs`, `main.rs`, `index.mjs`, `cli.mjs`) from the cross-module caller requirement. |
| **8** | **Coder Self-Review (`self_review.rs`, `coder.rs`, `finalization.rs`)** | Coder failed with `Bounded self-review correction did not establish current passing evidence` even when 100% of unit tests passed. | `self_review` lacked `run_test` output, flagged namespace imports as style findings, and `self.self_review` was never cleared when a coder correction only ran `run_test` without editing files. | Included `run_test` exit code + TAP output excerpt in `self_review::Request::prepare`, instructed `self_review` to return `findings: []` when all checks are met, and called `attempt.clear_self_review()` on `self-review` correction turns. |
| **9** | **Independent Reviewer (`reviewer.rs`)** | `Original Ask audit` failed when returning multiple `met: true` checks for multi-bullet asks, or broke unit-test retry bounds on invented obligations. | `grounded_audit` required `checks.len() == 1` even when all returned checks had `met: true`, and retried unconditionally on provenance errors. | Collapsed multi-check `verdict: "pass"` responses (where all checks have `met: true`) to the canonical 1-check audit schema, and restricted audit retry to schema/count errors. |
| **10** | **NLP Orchestrator & `start_sdlc_delivery` (`shell.rs`)** | `start_sdlc_delivery` ran the Planner twice and did not sync verified worktree files back to `target_repo`; Gate 1 checked `self.cwd` instead of `effective_repo_dir` and could fire on read-only questions. | `start_sdlc_delivery` called `run_ask_text_mode` instead of direct worktree delivery + `apply_verified_worktree_to_repo`, and Gate 1 lacked `is_delivery_intent` and `effective_repo_dir` scoping. | Wired `start_sdlc_delivery` to single-pass `execute_local_ask_delivery` + `apply_verified_worktree_to_repo`, scoped all 4 NLP completion gates to `effective_repo_dir`, and guarded Gate 1 with `is_delivery_intent`. |

---

## 3. Software Engineering Ask Archetype Matrix

| Archetype | Example Ask / Trigger | Primary Entrypoint | Seats / Loop Invoked | Deterministic Gates Enforced | Expected First-Try Outcome |
|---|---|---|---|---|---|
| **1. Shell Escape** | `!git status -sb` | `dispatch_line` (`!`) | Host shell (`execute_bang_shell_command`) | Secret redaction, 12k output bound | Immediate stdout/stderr + exit code in transcript |
| **2. Operator / Fleet / Board CLI** | `/fleet list`, `/route auto`, `/doctor openshell`, `/board 8` | `dispatch_line` (`/`) | Native CLI / Shell command handlers | Vault isolation, WSL 2 path resolution, GraphQL Project v2 validation | Deterministic status readout or board state transition |
| **3. Protected Surface Refusal** | `Edit agent-policy.yml to allow deploy` | `edit_file` / `write_file` / `Boundary::mutate` | Refused pre-write | `is_protected_agent_edit_path`, `forbidden_write`, `ROSTER_TOOL_DENIED` | Immediate refusal; zero bytes mutated on disk |
| **4. Read-Only Question & Repo Analysis** | `Inspect Directory.Build.props in Riddle...` or `What dev drives and repos are on this PC?` | `answer_repository_question` or `run_agentic_nlp_turn` | Read-only QA or NLP inspection tools (`list_dir`, `read_file`, `search_text`, `git_inspect`) | Read-only guard, 16-tool context retention, no Gate 1 false trigger | Evidence-grounded answer citing exact files and line numbers |
| **5. Compliance / Security Audit Report** | `Audit Riddle repo for SDLC & security gaps and save RIDDLE_AUDIT.md` | `run_agentic_nlp_turn` | `deep_inspect_repositories` + `write_report` | Secret redaction, real-file citation requirement | Board-ready Markdown audit report saved to disk |
| **6. Directory-Scoped Local Ask** | `Write a marketing design and setup guide. Output it in a new "new-design" directory` | `run_ask_text` -> `driver.run_local` | **Planner -> Coder -> Reviewer** in `.worktrees/local-<id>/` | Directory path containment, `Original Ask audit`, `REVIEW.md` pass | Verified deliverables in `.worktrees/local-<id>/new-design/` |
| **7. Bounded Code / Bugfix / Docs Slice** | `/run 9`, `roster run --issue N`, or `start_sdlc_delivery` | `execute_issue_delivery` / `driver.run_local` | **Planner -> OpenShell Prover -> Coder -> Reviewer** | Symbol grounding, dynamic exploration budget, `run_test`, `counterfactual` red/green, `shadow`, `self_review`, `Original Ask audit`, `Reviewer` | `Mode: verified`, `Review: pass`, verified diff synced & committed |
| **8. Greenfield E2E App + IaC + Tests** | `Build a zero-dependency Azure static site + Bicep + tests + checklist on Project #N` | `run_agentic_nlp_turn` | 48-turn agentic loop + **4-Stage Completion Gate** | Gate 1 (all files written), Gate 2 (`node_build`), Gate 3 (`node --test` exit 0), Gate 4 (commit/push + Kanban `Done`) | All source/IaC/doc/test files written, 100% tests green, board `Done` in 1 pass |
| **9. Multi-Card Kanban Board Run** | `/board run N` | `execute_project_board_run` | Sequential **Planner -> Prover -> Coder -> Reviewer** per card | Per-card 3-seat verification + issue close + Project v2 `Done` transition | Every `Todo`/`In Progress` card verified, committed, closed, and moved to `Done` |
