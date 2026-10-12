# Eve architecture and carry-forward audit

Audit for #473 and #489, updated 2026-10-12. Current reviewed baseline: PR #592
(`4de15f9558a117c183845e532e862664268fd9d2`), with contracts pinned to reviewed
`fa48c244bc9b8ead5df927ec705ef41a39c56f68`. Earlier verification records below
retain their original revisions and selectors. This is an implementation/evidence
inventory, **not** human acceptance, a release claim, or a reset of failed runs.

Product trace: FEATURE_SPEC **5.1–5.8**, with principles **3**, non-goals **7** and
harness acceptance **8** binding. Sources read directly: FEATURE_SPEC,
MULTIAGENT, SDLC, ONE_TASK_LOOP, METRICS, FLEET, source modules below, issue bodies
and existing merged-PR evidence. No comparator marketing claims are needed.

## Current delivery and remaining gates

The runtime remains planner -> coder -> matching tests -> read-only reviewer ->
bounded fresh-context repair -> explicit App publication -> acceptance/learning.
Cognitive inspiration is implemented through bounded evidence, obligations,
context, memory and recovery inside this lifecycle, not a biological brain,
general chatbot swarm, second queue or new runtime.

- **Independent verification:** #588 wires explicitly host-approved Docker
  observations into ordinary Ask, issue and existing-TASK review paths, including
  fresh execution after coder repairs. #592 adds bounded host-owned verifier
  sources on a separate read-only mount and explicitly approved unchanged
  dependencies. Exact TASK/RESULT/product bytes bind observations, review and
  final publication. Contradictions override model approval; invalid plans and
  backend failures are explicit failing/incomplete reviews. Defaults remain
  unchanged: no implicit image acquisition, executable discovery or model
  authority to approve its own oracle. Read-only source is not an unspoofable
  oracle against intentionally hostile product code.
- **Actual retained counterexamples:** a real Docker replay invokes a deliberately
  false-pass mock reviewer against the original faulty and correct M3 bytes.
  The faulty delivery passes 6/18 parser and 6/18 CLI cases and is rejected;
  the correct delivery passes 18/18 of each and is approved, then publication
  is denied after its unchanged parser dependency changes. Combined valid
  quoted-comma/escaped-quote/newline behavior passes both. Original retained
  files remain unchanged. This is host-approved test evidence, not live
  inference, autonomous counterexample discovery or human acceptance.
- **Publishing recovery:** reviewed contracts
  [R4yHarris/github-agent-contracts#13](https://github.com/R4yHarris/github-agent-contracts/pull/13)
  and Roster #589 deliver explicit exact-head `--resume` consumption without a
  replacement commit, push or PR. The original App/model/run identity, exact
  remote and PR head, current policy and required checks are revalidated,
  including ruleset-only protection. No vendor sources were rewritten here.
- **Pilot:** Ray explicitly accepted #474/#475. #590 exercises the real public
  baseline/opt-in lifecycle and paired evaluator with deterministic transport,
  actual fixture tests and honest unknowns. It is synthetic, not held out, and
  reports `pilot-incomplete`. #476 remains open for its separate held-out and
  comparative acceptance obligations; #477 remains gated. No 40-task/20-fault
  campaign or difficulty promotion is implied by these development controls.
- **Other gaps:** #439's trusted base-checker workflow source remains
  maintainer-owned carry-forward. Cost requires authoritative returned units or
  human-owned rates; unknown cost remains unknown. #570 is an offline-only
  Copilot feasibility lane under #569, not an installed or approved provider.
  #571-#573 retain their architecture, auth and live-call approval gates.

At exact #592 head `cd937349a3be32ee4278dac3adeb99090912a20d`, required
Linux Node20 CI passed **1947/1950** tests, zero failures, with **three explicit
Docker opt-in skips**, in **55593.740921 ms**. Windows Node `v20.20.2` ran the
actual Docker backend/receipt/reviewer gate: **47/47**, zero skips, in
**45595.4996 ms**. Ordinary public issue/repair controls passed **2/2 selected**
tests, with 17 intentionally unselected, in **26277.6279 ms**. These are exact
selectors and environments, not a controlled speedup or end-to-end model-quality
claim. Fresh protected-oracle M3 calibration is tracked separately in #593.

### Fresh calibration and review-only recovery

#593 ran the public M3 lifecycle at `4de15f9` using the saved requested model
`qwen3.8-27b`; the endpoint actually served `Qwen/Qwen3.8-27B-FP8`, effort `m`.
The fresh task was `local-9015e2d9ea61ce19`, elapsed **259532 ms**. Two matching
test repairs completed; final consumer tests passed **16/16**, including its
baseline, and protected Docker plus the independent post-run matrix passed
**18/18 parser and 18/18 CLI** cases. The original run still failed because
the reviewer was incomplete after malformed JSON and a request failure.
Publication was denied and `verified_delivery: false` remains unchanged.

A targeted read-only recovery, not another coder run, completed with a false
negative in **30525 ms**: it claimed an unclosed multiline quote at EOF would
not throw. Actual source calls `endField()` from `endRecord()` and throws while
`inQuotes`; the expanded host oracle executes that exact EOF input in both
parser and CLI. This led to two concrete library repairs:

- #594 forwards explicit host approval/backend options through `runOnlyReview`,
  uses its existing `again` archive path, and refuses to silently discard prior
  independent verification. A direct seat-only attempt before using this API
  failed `EEXIST`; no successful verdict was inferred from that failed attempt.
- #595 supplies exact, redacted, subject-bound host observation facts to both
  model review and Original Ask audit. Previously they affected the final gate
  but were absent from both model contexts. Passing observations still cover
  only declared inputs and do not override a separate model-reported defect.

With those repairs and the concrete EOF observation, one bounded reviewer-only
recovery passed in **39960 ms**, serving `Qwen/Qwen3.8-27B-FP8`. Its returned
review response reports **11251 prompt tokens / 471 completion tokens**; these
are the recorded response fields, not an independently measured aggregate bill
for every earlier request. Cost and unreturned counts remain unknown.
TASK/RESULT/product subject SHA-256 remained
`35b09b12b43c062d85b78fc4dc6034a0b344fa43f1fc6e475c422a3e5b6f8ecd`.
The passing review gate was checked; no consumer publication occurred.

Final Windows Node20 coupled review-only/receipt/reviewer controls:
**44/44**, zero failures/skips, **50291.6603 ms**, including actual Docker.
Ordinary public issue/repair controls: **2/2 selected**, 17 intentionally
unselected, **45858.4026 ms**. This documents failure-driven implementation
and assisted recovery, not a new autonomous accepted run, held-out pilot,
general semantic reliability, or authorization to promote difficulty.

## Architecture against the code

| Designed component | Current implementation evidence | Boundary / remaining gap |
| --- | --- | --- |
| App identity, role authority and provenance (5.1) | `lib/publication.mjs`, `seat-publication.mjs`, `metrics/run.mjs`; pinned contracts publisher; protected tools; reviewed exact-head recovery consumed in #589 | Publishing SDK/merger/deployer authority belongs to contracts/human policy, not a missing model seat. Explicit recovery preserves the original App commit/model/run; no human-credential fallback or vendor rewrite. |
| Trusted CI/review gates (5.5) | Read-only `seats/reviewer.mjs`, `runtime/review-verify.mjs`, bounded fresh-context review repair in `lib/builtin.mjs` | #439 remains real human-owned workflow debt: checkout does not independently select base-revision action/checker source. A green check alone does not establish that trust property. No workflow edits were made. |
| Planner → coder → reviewer seats (5.2–5.5) | `seats/planner.mjs`, `coder.mjs`, `reviewer.mjs`; sequential orchestration, cache/retry/repairs in `lib/builtin.mjs` | Implemented, not absent. Same process/worktree by default; opt-in isolated child/attempt trees are not a new runtime or task board. Stable Eve worker identity independent of seats/models is a later #477 outcome, not delivered by these names. |
| Ask taxonomy/specialization (5.3) | `planner/classify.mjs` produces question/incident `specKind` metadata; execution still uses clarify/slice/feature/initiative; `/btw` is a bounded read-only task side-question. `runtime/context-policy.mjs` consumes bounded incident metadata plus an explicit investigation/repair directive from the preserved TASK Ask to select the existing incident coder packet. | Incident specialization is guidance inside the ordinary gated seats, not a new primary runtime or urgency grant. Ambiguous incidents still clarify/split; questions or documentation mentions alone do not select the packet. Skill-bounded recipes must cover incident. Primary repository questions now use `lib/repository-question.mjs` via explicit CLI/shell and natural-question intake (#550), from human-declared read scope only and without executing a task. `/btw` remains a distinct task-metadata question. |
| Planner memory and continuity (5.6) | Planner reads at most 20 redacted seat-scoped records; writes Ask/title records; builtin appends review outcome; `runtime/memory.mjs`; merged #520/#523 | Advisory bounded memory, not full conversational replay or a global mind. Coder memory is separate and included in ordinary minimum packs (#515). |
| Skills/conduct/learning (5.2/5.6) | `runtime/context.mjs`, `skills.mjs`, `engineering-baseline.mjs`; `lib/learn.mjs` eval joins/ceilings; `failure-proposals.mjs` human-reviewed drafts | The five baseline packets remain; six installed spec category packets (`research`, `implement`, `test`, `review`, `incident`, `docs`) now reach explicitly selected minimum/full coder contexts. A required category cannot be budget-omitted; missing/malformed/junction packets stop preparation. Default baseline selection is unchanged. This does not make a coder an independent reviewer or by itself implement a primary handler; #550 separately delivers primary repository questions. Curated record/compaction primitives exist (`provenance-api.mjs`, `curateMemory`), but source-invalidating, seat-scoped promotion/retrieval/unpromotion is not an integrated Eve learning loop. This is explicit #477 work gated by #476. Proposals are not automatically installed principals or fabricated human accepts. |
| Fleet, routing and admission (5.2) | `lib/fleet.mjs`, `route.mjs`, `llm.mjs`, `runtime/admission.mjs`; eval history before priors, hardware/cost ranking, FIFO slots and abort release | Admission is per profile in one process, as FLEET documents; not distributed admission. Ordinary saved-default runs without a fleet profile are not admission-limited. Registered hardware experiments and cross-process resource guarantees are not proven by unit tests. Custom default key-name reload defect was reproduced and repaired in #540. |
| Issue/Project queue and dependencies (5.3/5.7) | `lib/issue-board.mjs` labels/status comments/optional Project projection; `waves.mjs` issue dependency and earlier-wave closure checks; `wave-scheduler.mjs` | GitHub is canonical. Project access is installation-permission-dependent; local state is evidence/recovery, not an independent Kanban DB. Child closure does not invent human acceptance. |
| Worktree lifecycle and liveness (5.4/5.8) | `lib/start.mjs` fresh base; issue preparation and reuse; `repo-locks.mjs` explicit death proof; `worktrees.mjs`, `local-runs.mjs`, `clean-ops.mjs` | #487/#491 repair lock takeover; age/unknown liveness is not death. Merged #541 moves generated task environment metadata under worktree Git storage and preserves any existing product `.env`. #552 coordinates the spec 5.4 branch namespace: new `agents/<issue>` branches, exact legacy `issue-N` reuse, stable task/directory/lock identity, dual-namespace refusal and compatible status/publication/checkpoint lookup. No destructive branch/worktree rename or human acceptance is implied. |
| Checkpoints/recovery (5.4/5.8) | `lib/checkpoints.mjs` guarded before-write Git tree snapshots and explicit rewind; validated handoff/worktree reuse; whole-task deadline and durable terminal evidence | Pre-write snapshots/manual rewind are not proof of automatic last-green-step model/context replay. Explicit identity-bound test-green restoration/resume is now implemented; unattended model/context replay remains partial; never silently restore stale product state. #539 adds safe durable ambiguous-intake continuation, not a claim to implement the entire recovery lifecycle. |
| Human CLI/shell and vague asks (5.7/5.8) | `cli.mjs`, `repl.mjs`, shell tray/history/context; #536 interpretation/questions; #539 private project intake checkpoint | Original redacted Ask, bounded guidance and validated explicit reply survive restart; `/clarify continue` / `ask --resume` reuse the reply. Shell resumes behind `--confirm`; cancellation revokes Enter/retry. No coding/publication from a vague ask, inferred confidence score, or silent override of original constraints. |
| Operations/provenance (5.8) | `task-deadline.mjs`, `run-log.mjs`, `local-runs.mjs`, typed machine history/provenance, `stranded.mjs`, vault, safe cleanup | Local logs/history and unknown-aware usage are implemented. Optional OpenTelemetry export is not wired. Cloud `cost_usd` schema exists, but no priced producer ledger is wired; hardware ranking is not a measured-dollar ledger. This needs authoritative provider-reported cost or human-owned rates, not guessed prices/energy. |

## Concrete debt and ownership

**#551 incident guidance specialization:** recognized bounded incident/regression
Asks with an explicit investigation/repair directive automatically select the
installed incident packet in minimum/full coder contexts. TASK Ask, observed
failure, checks and scope remain unchanged; skill-bounded recipes must cover
the category. Offline Node20 verification: 92 targeted context/skills/recipe/
coder-scope/publication tests, 91 pass and one existing skip; final incident-only
gate after adding conservative controls: 22 pass, zero failures. Actual model
request controls distinguish incident from ordinary guidance and exercise the
same write/final gate, denied internet/workflow requests, and cancellation
before/after guidance delivery. Required budget/missing-packet controls fail
before inference. FEATURE_SPEC **5.2/5.3/5.4/5.5**, honoring **3/7**; no new
runtime, production permission or human acceptance is claimed.

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
7. **Partial core interfaces:** unattended model-context replay needs a separately
   bounded design. Branch namespace compatibility is
   implemented by the bounded #552 increment, not an isolated rename. These
   are not delivered by the basic seat loop or by naming a `specKind`; planner/
   tool/loop integration needs bounded public-path tests; the #449/#452 ownership handoff is already completed. Do not
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

## Category packet delivery verification

Explicit Windows Node20 targeted gate: specialized packet/context tests, skills,
minimum context, recipe bounds, coder scope, builtin publication, repo locks and
clarification checkpoints: **157 tests, 156 pass, one existing Windows symlink
skip, zero failures**. Six minimum-context/category controls and the full-context
loader failed against old code. Green includes actual coder HTTP delivery,
missing/empty/oversized packet refusal before inference, junction defense,
required full/minimum budget refusal, unselected packet isolation and unavailable
internet tool refusal. No human quality/acceptance, hardware or full suite claim.

## Next bounded acceptance conditions after category packet delivery

The death-proof lock acquisition and durable clarification/session intake are
already implemented, not blank acceptance gaps. Existing lock tests preserve
live/unknown owners and take over only explicitly dead positive PIDs. This audit
does not claim cross-platform live acceptance from unit tests.

- **Delivered bounded primary question path (#550):** explicit CLI/shell
  questions and naturally classified question Asks return bounded line-numbered
  public source evidence from human-declared read scope. Tool-free model
  interpretation is separate and optional. No product writes, seats, tests,
  issue creation, publication or implicit URL scope; unavailable evidence stays
  unknown. This is repository explanation, not a general chatbot crew.
- **Ungated incident specialization:** explicitly authorized incident tasks must
  retain failing observations and unchanged scope/checks while selecting the
  incident procedure; actual repair/review/publication stays the ordinary gated
  loop. No emergency production or workflow access. Category delivery is a
  foundation, not incident-specific execution acceptance.
- **Delivered bounded recovery increment:** explicit CLI/shell green resume
  validates recorded task/scope/revision/owner identity, preserves later edits,
  retires stale passing evidence and pauses for confirmation. Real coder capture
  and separate CLI/default-shell process restart controls cover restoration,
  changed identity/ref, active lock, linked/malformed state and secret backups.
  Unattended model-context replay is not this acceptance condition and remains
  an explicit product/policy boundary, never inferred execution permission.
- **Branch compatibility increment (#552):** coordinated creation, lookup, reused
  legacy `issue-N` branches, issue status and publisher claims adopt `agents/<issue>`
  without renaming trees. Offline Git fixtures cover namespace ambiguity, stable
  lock/task identity, actual checkpoint restoration and publication lookup refusal.
  This is implementation evidence, not human acceptance or permission to move claims.
- **Input-gated optional operations:** no guessed priced producer ledger without
  authoritative returned cost or human-owned pricing input. Optional telemetry
  export is not a current delivery prerequisite and needs an explicit collector
  configuration; no new runtime dependency was added.
- **Human/dependency gates:** #474/#475 were explicitly accepted by Ray and
  closed, allowing bounded #476 development. Full pilot acceptance remains
  outstanding and gates #477; measured resource/council evidence is not replaced
  by deterministic fixtures. Ray approved #499's test policy and delegated its
  technical disposition: closed/Done after fresh controls, retaining the
  original timeout separately from successful unchanged minimal-consumer runs.
  #439 workflow source binding remains human-owned. Exact-head publisher
  recovery was delivered upstream and consumed through the reviewed gitlink,
  never a vendor-source edit here.

## Verified last-green recovery evidence

Spec §§5.4/5.8, honoring §§3/5.5/7: existing checkpoint trees and local issue
registration now provide an explicit last-green product restore/resume path.
No contracts, workflow, policy, installed checkout or queue changes. This is
actual product restoration, not printed guidance or intake-only continuity.

Node 20 targeted checkpoint/resume/CLI/REPL/coder scope coverage: **83 tests,
83 pass, zero skips/failures**. Reviewer/publication gate controls: **43/43 pass**. Twelve initial checkpoint identity/green controls
failed old code; new real coder checks capture checked products, and fresh
CLI/default-shell processes restore them behind confirmation. Failed/skipped
or absent evidence, post-check modifications, changed assignment/task/recipe/
HEAD/branch/scope/ref, active lock, cancellation, malformed/hardlinked/junction
metadata, unknown green and secret backup material fail closed. Later product
edits remain recoverable; stale RESULT/REVIEW is archived. No claim of live
cross-platform/hardware acceptance, human rating or unattended context replay.

Child #555 adds interruption controls for multi-file restoration. Cancellation
after any replacement, including the final write or deletion, enters the
existing rollback path instead of returning a successful checkpoint. Failed
restoration preserves exact prior product bytes and file absence, retains the
before-write backup for manual recovery, and releases the issue writer lock.
A rollback I/O failure remains an explicit error requiring product inspection;
the retained backup is recoverable, not a claim that rollback succeeded.
Interrupted shell restoration must not launch a seat or publisher. These are
bounded recovery controls, not unattended replay or human acceptance.

Windows Node20 verification for #555: inherited interruption controls reproduced
six passes and two failures before repair (final-write/final-delete cancellation
returned success). After integrating reviewed #552 merge `6de6d358`, combined
interruption/checkpoint/identity controls passed **39/39**, zero failures/skips;
separate fresh CLI/shell green-resume and builtin publication safeguards passed
**20/20**, zero failures/skips. No full-local-suite, new remote CI, publication,
independent acceptance or model-quality result is implied by these local checks.

The parent development-review continuation integrated main `bfdb9aa9` and
replayed interruption, checkpoint, identity, green-resume, builtin resume and
publication controls: **72/72** passed on Windows Node20, zero failures/skips.
This is additional offline verification, not human acceptance or a live task result.

## Bounded repository question implementation evidence

Tracked before resumed implementation as child #550 / Build In progress. Scope
is question helper, Ask routing, CLI/REPL question entrypoints and tests/docs;
shared lines coordinated with #551/#552 owners, no incident/branch/checkpoint
changes. Spec §§5.3/5.7, respecting §§3/7. No new runtime dependency/queue.

Offline controls include actual CLI process restart, default shell routing,
source line attribution, no seat/GitHub/state writes, explicit scope only,
URL non-authority, key redaction, malformed/private/linked/unavailable source,
entry/file/byte/context/output caps, model tool-call refusal and cancellation.
No hardware/human acceptance or autonomous answer correctness is inferred.

Windows Node20 question/intake/CLI/REPL/classification/side-question controls: **104/104 pass, zero skips/failures**. Includes streaming single-line shell across fresh sessions, CLI subprocess restarts and explicit human executable contract precedence; model interpretation is not independently accepted answer correctness.

After integrating reviewed incident main `b14f2eb`, the same question/interface suite plus incident-selection controls passed **126/126**, zero skips/failures. Incident evidence above is retained; no incident selection or branch/recovery ownership change is included in the question slice.
