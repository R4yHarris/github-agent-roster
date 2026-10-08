# Software engineering lifecycle for Roster seats

This document grounds Roster's agent lifecycle in how a good software engineer
actually delivers an ask. It refines [FEATURE_SPEC §4](FEATURE_SPEC.md#4-lifecycle-the-harness-must-run)
into steps, each with an owner, a narrow input, one output artifact and a gate.
The goal is focused LLM turns: each turn gets only what that step needs, has one
job, and is checked by code rather than by prose.

Tracking epic: #293.

## Why this shape

Every failed slice in the #200 history waves (#284-#286) failed at a step a
human engineer would not skip:

- the coder didn't look for an existing module before creating a new one;
- the plan's checks named concepts, not code, so nobody could verify them;
- the tests passed but never proved the change;
- review judged prose evidence and never ran the code.

The roles (plan, code, review) were right. The discipline between them was
missing. The popular harnesses (Claude Code, Codex CLI, Copilot, Hermes,
OpenClaw) enforce that discipline in tools and gates. Prompt text only frames
the turn.

## Principles for LLM input

1. **One step, one job.** A turn gets the inputs for its step and returns that
   step's artifact. It does not also plan, code and review.
2. **Context is curated, not dumped.** Each seat gets the rule layers and
   conventions that bind it, never the whole AGENTS.md (#294, #301).
3. **Enforce, then explain.** If a step must happen (read before writing,
   search before creating, red before green), a tool or gate refuses to
   continue without it. The refusal message names the exact missing action.
4. **Artifacts are the memory.** TASK.md, the Design, the checklist, RESULT.md
   and REVIEW.md carry state between turns and seats. The transcript does not
   (§3).
5. **Bounded loops.** Each repair loop has a fixed budget and changes strategy
   on repeated failure (§5.4).
6. **Measure every step.** Time, tokens, repairs and defects per step feed
   routing and estimation (§5.6).

## Refined lifecycle

A good engineer's process is:

> familiarity -> rules -> understand the ask -> breakdown -> estimate ->
> checklist -> outline -> design -> tests -> code -> remediate -> review ->
> review again -> deliver -> deploy

Refined, and with the steps engineers do implicitly made explicit, it becomes
the lifecycle below.

| # | Step | What a good engineer does | Roster owner | Output | Gate (deterministic) | Today | Gap |
|---|---|---|---|---|---|---|---|
| 0 | **Familiarity** | Knows the languages, layout, naming, idioms, test style and core modules | Harness (cached per base revision) | Repo map + conventions pack | Pack regenerates when the base revision changes | Filename-only repo map; public seams | #301 |
| 1 | **Rules** | Works within company, org, team and personal rules, in that order of precedence | Harness | Layered rules, each labeled with its source | Policy and protected paths are enforced by tools, never only stated | Whole AGENTS.md + principal | #294, #301 |
| 2 | **Intake (Definition of Ready)** | Restates the ask, asks when unclear, refuses unbounded work | Harness classifier | Ask kind: clarify, slice, feature or initiative | Unclear scope stops at `clarify` | Done ([SDLC](SDLC.md)) | – |
| 3 | **Locate (spike)** | Finds the code that already does part of this before planning | Harness search, then planner | Relevant definitions, not just names | Ask nouns are grepped; matches are passed to the planner | Done: definition bodies in the slice planner prompt | #296 |
| 4 | **Breakdown** | Splits work into slices with one outcome, allowed files and checks each | Planner | PLAN.md, child issues, TASK.md | Plan validation; epics split before any coder starts | Done (waves) | – |
| 5 | **Estimate** | Sizes difficulty, minutes and confidence; re-estimates when scope grows | Planner | ESTIMATE.md + TASK metadata | Recorded before coding | Done ([ESTIMATION](ESTIMATION.md)) | – |
| 6 | **Acceptance checks** | Writes checks that can be verified against real code | Planner | TASK.md checks | Named existing symbols must exist | Done: backticked names are checked; a bad cite triggers a repair | #296 |
| 7 | **Design and outline** | Picks modules to extend, sketches signatures and pseudo-code, lists edge cases and risks | Planner (or the coder's read-only first turn) | TASK.md `Design` section | Design must name real modules; new exports go in allowed files | Done: validated or derived; coder re-validates before writing | #302 |
| 8 | **Checklist and todos** | Turns checks and Definition of Done into a todo list and works one item at a time | Coder | Checklist (tool state) | Can't finish with open items | Done | #297 |
| 9 | **Author tests (red)** | Writes the acceptance test first and sees it fail for the right reason | Coder | Test files | New tests must fail against base src | Done | #303 |
| 10 | **Author code (green)** | Reads the code before changing it, extends existing modules, makes the smallest complete change | Coder | Diff | Read before write; search before a new `src/` module; scope and secret guards | Scope and secret guards only | #295 |
| 11 | **Remediate and repeat** | Uses each failure as evidence, changes approach, stops at a budget | Coder loop | Green targeted tests | Bounded repairs; perspective escalation | Done ([LOOP](LOOP.md)) | – |
| 12 | **Self-review** | Reads own diff against the checklist before asking anyone else | Coder's model, fresh context, no write tools | Per-check self-review | One bounded repair on findings | Done | #304 |
| 13 | **Automatic gates** | Lint, CI, "is this a duplicate of something we have?" | Harness | Gate findings | Shadow-module gate; red/green proof; scoped tests | Done | #299, #303 |
| 14 | **Independent review** | A peer reads the code, runs it, and reports only verified findings | Reviewer seat (another model), read-only tools + harness-run end-to-end command | REVIEW.md (strict schema) | Fail if the end-to-end run fails; capped output | Done | #298 |
| 15 | **Review again** | Repairs findings and gets the second look | Coder, then reviewer | Updated diff + REVIEW.md | At most two review repairs; checks are never weakened (§5.5) | Done | – |
| 16 | **Deliver** | Opens the PR with provenance; merges when required checks pass | App publisher | PR with trailers, merged when green | `check-agent-trailers`; App identity only | Done | – |
| 17 | **Deploy** | Ships with a separate approval | Human / deployer role | Release | Separate grant, off by default (§5.5) | Out of scope by default | – |
| 18 | **Accept and learn** | Retro: what was estimated vs actual, what defects escaped | Human AI-Eval + ledger | Eval row, skill or principal update | No self-acceptance (§5.6) | Eval ledger; manual skill notes | #293 exploration |

## Grounded planning and Design

With an LLM configured, the slice planner receives real definition bodies for
the exports most related to the Ask (`src/planner/grounding.mjs`). Backticked
code names and paths in acceptance checks and the Design must exist in tracked
code, appear in the Ask, or be declared under New exports. A plan that cites
anything else is returned to the planner once with the reasons.

Every code slice gets a `## Design` section before Files allowed. It lists:

- **Extend:** existing modules and the exports to reuse.
- **New exports:** each new export, in an allowed file, that does not duplicate
  an export elsewhere, even when the existing owner's file is also allowed.
  Include that owner under Extend instead; scope permission is not evidence
  that a second implementation is needed.
- **Outline**, **Edge cases** and **Out of scope.**

If the planner gives no Design, or an invalid one, the harness derives one from
Files allowed and the related exports, and records the rejection reason. The
coder re-validates the Design against its worktree and refuses before the first
write if it no longer holds. A hand-written TASK.md without a Design is still
accepted. The reviewer reports drift from the Design. Docs-only slices have no
Design.

### Plan critic

Before a slice enters the coder, the harness critiques TASK.md, including reused
and explicitly accepted handoffs (FEATURE_SPEC
sections 5.3 and 5.5). The cheap deterministic pass reuses the tracked symbol
index and checks for ungrounded citations, duplicate Design exports, undeclared
missing allow-list files, and explicit assertions that an existing named test
already exists/passes. A test name is not proof that the requested behavior is
already implemented: ordinary instructions to extend an existing test are not
flagged. New paths must be declared in Design's New exports or a `## New files`
list before Files allowed/Ask. Wildcard scope retains its existing semantics.

If defects exist and an LLM planner is configured, it receives exactly one
critic revision, without app-code or artifact-write tools. The harness validates
that the Ask is unchanged and scope has not widened before writing planning
artifacts. Remaining defects are placed in `## Critic notes` for coder/reviewer
visibility; they are advisory, not an unbounded replanning loop. Structural plan
validation and protected-surface gates remain authoritative. Stub runs annotate
defects without contacting a model.

An optional `planner.critic_profile` names a fleet profile for a second,
read-only model pass. It must differ from the planner's profile and endpoint/model
pair; missing or non-independent profiles fail explicitly. No profile is chosen
silently. The critic receives the unchanged Ask/checks/Design and bounded real
definitions, has an empty tool list, and returns only
`{"defects":[{"check":1,"problem":"...","fix":"..."}]}` (null for plan-wide defects).
At most eight model findings, 240 characters per problem/fix, a 600-token
completion cap, and no length-expansion retry. Malformed output or endpoint
failure stops explicitly, never masquerading as an empty successful critique.
Response-backed critic passes and the planner revision are journaled separately;
they never replace the original planner's measured model or usage. Standalone
coder tasks without a planner retain their existing Git-independent path.

### Field contracts

The coder's context also gets a `## Field contracts` section
(`src/runtime/field-contracts.mjs`). It lists the record fields that the
allowed source files read, plus camelCase names in the Ask and checks. For each
field it cites up to three `file:line` lines in `src/` that state a contract:
a `must` message, a thrown error, a `typeof` check or a pattern test. Only
fields with at least one `must` line are kept. Fields the task names come
first. The section is about 1500 characters, redacted, and dropped before
required sections when the pack is full. This exists because a coder once
treated `repoIdentity`, a derived hash, as a filesystem path.

### Check triage

Before the coder's first turn, `src/runtime/check-triage.mjs` sorts each
acceptance check into met, unmet or unknown, with evidence. It makes no model
or network call:

- **Test commands.** A backticked `node --test <file>` runs only the named
  test files. A file that does not exist yet, or a test run that fails, makes
  the check unmet.
- **Exports.** A backticked `` `name` from `path` `` is looked up in that
  module. A missing module or export makes the check unmet.
- **Prose.** Anything else stays unknown.

A check is met only when it is just a passing test command. The evidence is
added to CONTEXT.md as `## Check triage`. When every check is met, the slice
becomes tests-only: Files allowed shrink to the test files, and any product
code edit fails the candidate with a `Check triage:` reason. This exists
because a run once spent 250 lines on checks that earlier waves already met.

## Checklist

When TASK.md has acceptance checks and more than one allowed file, the coder
gets an `update_checklist` tool (`src/runtime/checklist.mjs`) seeded with one
item per check. It marks one item `in_progress` at a time, and each `done`
or `blocked` item needs evidence. After the first update, a final answer with
open items is returned with the open list, at most twice. A coder that never
calls the tool is not corrected; lines such as `1. done: evidence` in its final
summary close those items. RESULT.md gets a per-check table, the run log
records `checklist d/t`, and the status rail shows `✓d/t`.

## Red/green

Once a coder candidate passes its checks, `src/runtime/red-green.mjs` runs only
the changed test files twice. It runs them once in a temporary base worktree at
HEAD with the candidate's changed `tests/` files overlaid (old product files,
new tests), and once in the worktree. The worktree is never written. A test
counts as new when its literal `test()` or `it()` name is absent from the
base version of the file. Each new test must fail at base and pass on the
candidate. A new test that passes at base is "not red": the coder gets one
correction, then the evidence is recorded either way. RESULT.md gets a
`## Red/green` table and the run log records `red-green <status> tests=N
not-red=M`. A TASK.md with `tests: characterization` (a pure refactor or
test-only task) is exempt, and `tests: none` skips it. A change that touches
only test files is also exempt, since no product code is there to make a test red.

## Shadow modules

After red/green, `src/runtime/shadow-modules.mjs` compares the exports the
candidate adds to changed product files (compared with `git show HEAD:file`)
against every other tracked `src/`, `bin/` and `scripts/` module. It is
deterministic and costs no model call. An export with the same name as one in
another module is always flagged. For a new file, an export is also flagged
when its role verb (read, write, validate, digest, parse, format, redact)
matches a peer export with the same nouns, or when the existing module's file
name already covers those nouns. For example, `readRecords` in a new module
next to a store that exports `readRecords` gets flagged. Re-exports
(`export { x } from`) are not new definitions. Findings return to the coder
once as `Shadow module:` reasons (import or extend the existing module, or say
why a new one is needed), and the reviewer then sees the evidence.
Exports from earlier waves of the same epic are labelled. A new export that no
product module uses (only tests, or nothing) is flagged as well, unless the
task text names it or it lives in an entry point (`src/cli.mjs`, `bin/`). For
example, a `pathsEqual` helper called only by its test gets flagged. This
catches helpers written to satisfy a check but never wired into the delivered
path. RESULT.md gets a
`## Shadow modules` section, and the run log records `shadow-modules <status>
findings=N`.

## Self-review

After the shadow-module gate, when the diff changes product code, `src/runtime/self-review.mjs`
runs one fresh-context turn on the coder's model with no tools. Its input is
the numbered TASK checks, the Design (if any), the red/green table, and the
diff. It must return JSON with one `{id, met, evidence}` entry per check and at
most eight one-line findings (debug leftovers, missing error paths, naming
drift, changes outside the Ask). Unmet checks or findings return to the coder
once as `Self-review:` reasons. It runs only once per coder seat, and the
independent reviewer judges the repair. A tool request, invalid JSON, or an
endpoint error is recorded as `unavailable`, and the reviewer still runs.
RESULT.md gets a `## Self-review` section. The run log records `self-review
<status> unmet=N findings=M ms=T in=I out=O`, and its tokens count toward the
coder's usage. The coder memory record keeps a `self_review` summary so
repeated misses can become skills (§5.6). Docs-only and test-only diffs go
straight to review.

## Verifying reviewer

The reviewer still has no function tools and can't write. Every turn is a strict
`json_object` response. To confirm evidence it may answer with
`{"requests": [...]}`: one to four `read_file`, `search_text`, or `git_diff`
requests, for up to three rounds. The harness answers them through the coder's
read guards, which refuse secrets, logs, and paths outside the worktree, and
refuses any other tool, such as `write_file`. Then the reviewer returns its verdict.

Before the reviewer turn, `src/runtime/review-verify.mjs` runs up to three
read-only `roster` commands that the checks name in backticks. Only
`history list|show`, `status --offline`, `recipe validate`, `stats`,
`recommend`, `doctor`, and `fleet list` qualify; `ask`, `run`, publishing,
probes, and the vault never do. Each command runs on the worktree's own data
under a temporary HOME with no GitHub credentials. Each one also runs once with
an unknown option, which is the error case. The review fails deterministically,
whatever the model says, when any of these happens:
- a command exits non-zero, unless its check expects an error;
- a command crashes with a stack trace or times out;
- a command prints invalid JSON when its check or flags ask for JSON (the #285
  warning-before-JSON flaw);
- a command accepts the unknown option.

The output goes into the evidence as `## Harness end-to-end runs`, and the run
log records `review-e2e <status> commands=N failures=M` and `review-reads count=N
refused=M`. The verdict JSON keeps its exact keys (`verdict`, `reasons`,
`security_notes`, `checks`, and optional `defects`) and is capped at 6000 characters. Extra keys, prose,
or a longer answer get one correction, then the review is incomplete and fails.

## Seat turn contracts

Each seat turn is a contract: narrow input, allowed tools, one output, and a
done condition the harness checks.

| Seat turn | Input | Tools | Output | Done when |
|---|---|---|---|---|
| Locate (harness) | Ask, repo map, conventions pack | Search only (deterministic) | Matching definitions | Always (no LLM) |
| Planner | Ask, rule layers 1-3, located definitions, history | None | PLAN/TASK with checks + Design | Plan validates; named symbols exist |
| Coder | Rules 3-6, conventions, TASK + Design, checklist | Read, search, edit/write in scope, scoped test | Diff + RESULT.md | Checklist closed; red/green proven; targeted tests green |
| Self-review | Checks, Design, diff, test evidence | None | Per-check verdict | Schema-valid; findings get one repair |
| Gates (harness) | Diff, base revision | Deterministic | Findings | No duplicate modules; red/green; scope |
| Reviewer | Checks, Design, diff, gate results, end-to-end output | Read-only | REVIEW.md (strict schema) | Pass, fail or escalate with evidence |
| Publisher | Reviewed diff | App SDK | Merged PR | Required checks green |

## Implementation order

Order the #293 slices so each one makes the next more effective:

1. #295: read before write and search before create (the cheapest stop to invented modules).
2. #300: remove unneeded coder tools (less to choose from, fewer wasted turns).
3. #294 + #301: seat-scoped input with conventions and layered rules.
4. #296 + #302: grounded checks and a design outline.
5. #297: checklist tool.
6. #303: red/green gate.
7. #304 + #299: self-review and the shadow-module gate.
8. #298: reviewer that verifies.

Each slice is measured before and after on a dogfood run: reads versus writes,
turns, repairs, review catch rate, and the number of GHCP fixes needed after
review.
