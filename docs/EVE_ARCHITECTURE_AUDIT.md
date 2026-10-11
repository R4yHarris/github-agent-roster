# Eve architecture and carry-forward audit

Audit for #473 and #489, 2026-10-11. Baseline: reviewed main through PR #541
(`24707b9f1bb78df90decb76e297027968389c23c`). This is an implementation/evidence
inventory, **not** human acceptance, a release claim, or a reset of failed runs.

Product trace: FEATURE_SPEC **5.1–5.8**, with principles **3**, non-goals **7** and
harness acceptance **8** binding. Sources read directly: FEATURE_SPEC,
MULTIAGENT, SDLC, ONE_TASK_LOOP, METRICS, FLEET, source modules below, issue bodies
and existing merged-PR evidence. No comparator marketing claims are needed.

## Architecture against the code

| Designed component | Current implementation evidence | Boundary / remaining gap |
| --- | --- | --- |
| App identity, role authority and provenance (5.1) | `lib/publication.mjs`, `seat-publication.mjs`, `metrics/run.mjs`; pinned contracts publisher; protected tools | Publishing SDK/merger/deployer authority belongs to contracts/human policy, not a missing model seat. #368 publisher stale-head recovery remains external debt; use single-commit branches, not vendor edits. |
| Trusted CI/review gates (5.5) | Read-only `seats/reviewer.mjs`, `runtime/review-verify.mjs`, bounded fresh-context review repair in `lib/builtin.mjs` | #439 remains real human-owned workflow debt: checkout does not independently select base-revision action/checker source. A green check alone does not establish that trust property. No workflow edits were made. |
| Planner → coder → reviewer seats (5.2–5.5) | `seats/planner.mjs`, `coder.mjs`, `reviewer.mjs`; sequential orchestration, cache/retry/repairs in `lib/builtin.mjs` | Implemented, not absent. Same process/worktree by default; opt-in isolated child/attempt trees are not a new runtime or task board. Stable Eve worker identity independent of seats/models is a later #477 outcome, not delivered by these names. |
| Ask taxonomy/specialization (5.3) | `planner/classify.mjs` produces question/incident `specKind` metadata; execution still uses clarify/slice/feature/initiative; `/btw` is a bounded read-only task side-question | `specKind` is not otherwise consumed by runtime code. Top-level question/incident-specialized execution remains partial; do not equate metadata labels or `/btw` with a full primary Ask handler. |
| Planner memory and continuity (5.6) | Planner reads at most 20 redacted seat-scoped records; writes Ask/title records; builtin appends review outcome; `runtime/memory.mjs`; merged #520/#523 | Advisory bounded memory, not full conversational replay or a global mind. Coder memory is separate and included in ordinary minimum packs (#515). |
| Skills/conduct/learning (5.2/5.6) | `runtime/context.mjs`, `skills.mjs`, `engineering-baseline.mjs`; `lib/learn.mjs` eval joins/ceilings; `failure-proposals.mjs` human-reviewed drafts | Only five generic skill packets are shipped (`implement-task`, `read-before-write`, `result-report`, `run-tests`, `small-diff`), not all the research/review/incident/docs-specific packets listed in 5.2. Curated record/compaction primitives exist (`provenance-api.mjs`, `curateMemory`), but source-invalidating, seat-scoped promotion/retrieval/unpromotion is not an integrated Eve learning loop. This is explicit #477 work gated by #476. Proposals are not automatically installed principals or fabricated human accepts. |
| Fleet, routing and admission (5.2) | `lib/fleet.mjs`, `route.mjs`, `llm.mjs`, `runtime/admission.mjs`; eval history before priors, hardware/cost ranking, FIFO slots and abort release | Admission is per profile in one process, as FLEET documents; not distributed admission. Ordinary saved-default runs without a fleet profile are not admission-limited. Registered hardware experiments and cross-process resource guarantees are not proven by unit tests. Custom default key-name reload defect was reproduced and repaired in #540. |
| Issue/Project queue and dependencies (5.3/5.7) | `lib/issue-board.mjs` labels/status comments/optional Project projection; `waves.mjs` issue dependency and earlier-wave closure checks; `wave-scheduler.mjs` | GitHub is canonical. Project access is installation-permission-dependent; local state is evidence/recovery, not an independent Kanban DB. Child closure does not invent human acceptance. |
| Worktree lifecycle and liveness (5.4/5.8) | `lib/start.mjs` fresh base; issue preparation and reuse; `repo-locks.mjs` explicit death proof; `worktrees.mjs`, `local-runs.mjs`, `clean-ops.mjs` | #487/#491 repair lock takeover; age/unknown liveness is not death. Merged #541 moves generated task environment metadata under worktree Git storage and preserves any existing product `.env`. Literal branch naming also diverges: spec 5.4 says `agents/<issue>`; builtin preparation/board publication use `issue-N`. A coordinated compatibility migration is needed, not an isolated namespace rename. |
| Checkpoints/recovery (5.4/5.8) | `lib/checkpoints.mjs` guarded before-write Git tree snapshots and explicit rewind; validated handoff/worktree reuse; whole-task deadline and durable terminal evidence | Pre-write snapshots/manual rewind are not proof of automatic last-green-step model/context replay. Automatic green-step recovery remains partial; never silently restore stale product state. #539 adds safe durable ambiguous-intake continuation, not a claim to implement the entire recovery lifecycle. |
| Human CLI/shell and vague asks (5.7/5.8) | `cli.mjs`, `repl.mjs`, shell tray/history/context; #536 interpretation/questions; #539 private project intake checkpoint | Original redacted Ask, bounded guidance and validated explicit reply survive restart; `/clarify continue` / `ask --resume` reuse the reply. Shell resumes behind `--confirm`; cancellation revokes Enter/retry. No coding/publication from a vague ask, inferred confidence score, or silent override of original constraints. |
| Operations/provenance (5.8) | `task-deadline.mjs`, `run-log.mjs`, `local-runs.mjs`, typed machine history/provenance, `stranded.mjs`, vault, safe cleanup | Local logs/history and unknown-aware usage are implemented. Optional OpenTelemetry export is not wired. Cloud `cost_usd` schema exists, but no priced producer ledger is wired; hardware ranking is not a measured-dollar ledger. This needs authoritative provider-reported cost or human-owned rates, not guessed prices/energy. |

## Concrete debt and ownership

1. **#449, repaired after explicit ownership handoff:** full and scoped runs now
   compare outside failures against the unchanged base. Only measured base
   failures enter `preexisting_files`; unavailable/infrastructure/interrupted
   comparisons and the eight-file cap retain `unclassified_files` plus a bounded
   diagnostic. Loop excuses require explicit measured pre-existing evidence;
   prior writes or unclassified status override it. Node20 targeted gate: 88
   tests, 87 pass, one existing Windows symlink skip, zero failures. Original
   administrative closure and failed model attempts remain historical evidence.
2. **#452, repaired after explicit ownership handoff:** reference URLs no longer
   produce inferred local file/directory authority. Scheme/protocol-relative/www
   references, paths, quoted queries/fragments and remote Markdown link labels
   are excluded, including balanced nested and malformed inline links. Original
   Ask evidence is retained, with no fetch or internet grant. Explicit human
   scope, ordinary local mentions and validated output directories still bound
   planning. Link-only vague asks clarify instead of entering a coder. Node20
   targeted planner/intake/checkpoint/public Ask gate: 144 tests, 143 pass, one
   existing skip, zero failures. Historical failure/closure is not acceptance.
3. **#442, repaired in merged #541:** the fresh-consumer regression proved that
   preparation itself introduced a protected product `.env`. Generated task IDs
   now live under worktree Git metadata via the existing PER_WORKTREE scoped-state
   API, with guarded private creation/reuse. Existing consumer `.env` remains
   untouched and protected; no ignore-file workaround or consumer-secret reads.
   Retained candidate inspected before repair; fresh-consumer, reuse and unsafe
   storage regressions pass. Merge SHA: `24707b9f1bb78df90decb76e297027968389c23c`.
4. **#440:** two offline regressions demonstrated `CUSTOM_KEY` becoming
   `ROSTER_API_KEY` after both public config round-trip and separate CLI default
   selection/reload. **Merged #540** fixes only fleet selection's preset identity;
   legacy named presets, runtime profile metadata and policy remain intact.
5. **#439:** human workflow source-pinning decision/repair required (5.5 and 3).
6. **#368:** contracts publisher recovery; outside this repo's editable boundary.
7. **Partial core interfaces:** primary question/incident specialization, the complete
   class-specific skill packet set, automatic last-green-step recovery, and
   literal branch namespace compatibility need separately bounded designs. These
   are not delivered by the basic seat loop or by naming a `specKind`; planner/
   tool/loop integration overlaps the concurrent live-repair ownership. Do not
   grant permissions, restore product state or rename published branch claims
   speculatively. Optional telemetry and priced cloud ledger need explicit
   collector/pricing input; unavailable measurements must stay unknown.
8. **#477 follow-up architecture:** stable identity, curated engineering memory,
   actual resource/liveness experiments, alternatives/escalation experiments and
   one measured-benefit council specialization depend on #476. Planning is not
   permission to bypass that human-evaluated integration dependency.

## #489 carried-forward requirement reconciliation

“Present” means code/merged evidence exists; it does not close the original
acceptance debt. Historical issue closure was administrative and remains so.

| Historical requirement | Reconciliation on reviewed main |
| --- | --- |
| #469 engineering mission/instructions | Present: `engineering-baseline.mjs`, context and builtin seat transport; #515/#518. Targeted baseline/context evidence, not model reliability or human acceptance. |
| #468 ambiguous lock ownership/liveness | Present: explicit false death probe only in `repo-locks.mjs`; #487/#491. Unknown and probe errors preserve locks. |
| #465 whole-task deadline/recovery | Present: `task-deadline.mjs`, builtin lifecycle provenance; #506/#507. Cooperative cancellation, not rollback or killing a healthy model by age. |
| #464 Node20 TAP totals/named results | Present: evidence parser and typed verification records; #504/#505. Missing counts remain unknown. |
| #457 actual syntax preflight | Present: `tools.mjs` syntax check with separate actual-result coverage in `tools.syntax-preflight.test.mjs`; no claim of an unstarted suite. |
| #453 syntax/module-load repair excerpts | `testFailureEvidence` in `tools.mjs` retains bounded failure excerpts; original failing-case replay remains acceptance debt, not discharged by existence of a helper. |
| #452 URL-derived product scope | **Repaired after handoff**, URLs and remote link labels excluded from inferred authority; explicit/local/directory scope and original reference evidence preserved, 143 targeted Node20 passes/one existing skip. |
| #451 reviewer full-gate claims | Read-only reviewer/e2e evidence exists; review/provenance records distinguish executed checks. Original full-gate claim case still needs replay; scoped prose alone is not full-suite evidence. |
| #450 preservation controls plus red-green proof | `red-green.mjs` has actual base/candidate test evidence; original preservation-control scenario remains to replay against unchanged expectations. |
| #449 scoped regression/unavailable baseline | **Repaired after handoff**, unavailable/capped/interrupted comparisons retain unknown status; full and scoped real candidate/base controls, strict loop exemptions and prior-write overrides pass. |
| #448 green repair summary handoff | `loop.mjs` tracks tests and summary-only continuations; original tool-free handoff replay remains acceptance debt. |
| #447 Ask-first reasoned planning | Ask-preserving planning/critic/design/grounding paths plus #512/#515/#531/#538; original integrated delivery cases are not replaced by planner artifacts. |
| #446 redacted indexed/assignment syntax | Dedicated redaction/evidence routines exist; original redaction regression must remain an explicit check, not a claim of semantic preservation from a green CI badge. |
| #445 scoped deadlines/final gate | Separate transport/test/task limits exist; #499 minimal-consumer diagnosis resolves test-harness placement, not permission to disable final verification. Keep original timeout as failed evidence. |
| #444 withheld discovery selection | Merged #524 bounded correction, preserving denied-tool boundaries. |
| #443 Ask-to-integrated-software delivery | Real coder/reviewer/repair loop exists, but #414–#438 and #476 integrated acceptance remain open; artifacts/model prose are not product acceptance. |
| #442 generated task environment | **Merged #541**, generated metadata isolated in worktree Git storage; existing product env untouched/protected, not an env-ignore workaround. |
| #440 fleet-default key env name | **Merged #540**, public round-trip and fresh CLI regressions; named preset behavior retained. |
| #439 independently trusted checker | **Human-only unresolved**, current action/checker follows the checkout; base SHA selects the commit range, not checker source. |
| #368 publisher stale-head recovery | **External dependency debt**, single-commit publication workaround only. No vendor edits. |
| #195 durable history/provenance | Typed store/query/export/redaction/recovery and builtin lifecycle writers exist; reviewed METRICS explains failure/cancel/pause records. Remaining historical acceptance must be verified, not replaced with a new store. |
| #194 machine/repo/worktree state separation | Scoped paths/state APIs, migration ownership/liveness checks and cleanup exist; do not copy raw history or infer completion from administrative closure. |

## Retained candidates inspected read-only

`delivery-driver-20261009`, `coder-first-experiment-20261009`, and its issue-440,
issue-441 and issue-442 trees remain dirty and were not changed. The fleet and
task-environment diffs were read before fresh isolated repairs. Unrelated
candidate config changes and experimental coder-first behavior were not copied.
#491 already supplies the death-proof lock fix; #515 and #507 supply the mission
and deadline foundations. Original candidates/history remain available.

## Verification and rollout gates

- Windows, explicit Node 20 executable, targeted component tests only:
  admission, LLM admission, planner memory, memory/compaction, locks, whole-task
  deadline, engineering baseline, provenance API/local run provenance, syntax
  preflight and read-only reviewer verification: **152/152 pass**, zero skips.
- #539 clarification-related targeted regressions: **184/184 pass**; App PR
  merged after CI. #540 fleet/config/onboarding/metrics: **74 pass, two existing
  skips**, zero failures; red failures retained in its PR body.
- #541 task-environment/preparation caller regressions: **160/160 pass**, then
  **35/35 core preparation tests** after rebase; merged App CI. Fresh-consumer
  regression failed against baseline before the fix.
- A full local suite was not run; remote CI is separate evidence. No AI-Eval,
  hardware measurements, confidence numbers or accepted outcomes were invented.
- #473 remains **12/18 closed child issues**. #474/#475 are genuine human
  evaluation gates; #476 depends on them, and #477 depends on #476. #489 remains
  open because the concrete debt above is not discharged by this audit.
- #499's latest canonical comment records successful unchanged minimal-consumer
  E3 runs (55s and 64s) and no verification-policy change. Keep it open for its
  explicit human review; no fabricated scope decision or evaluation was made.
- #414–#438 remain an acceptance campaign, not automatically completed by unit
  regressions or M3 development diagnostics. Cross-platform, actual hardware,
  integrated delivery and genuine human evaluation still require real evidence.
