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
| 13 | **Automatic gates** | Lint, CI, "is this a duplicate of something we have?" | Harness | Gate findings | Shadow-module gate; red/green proof; scoped tests | Red/green done; shadow gate missing | #299, #303 |
| 14 | **Independent review** | A peer reads the code, runs it, and reports only verified findings | Reviewer seat (another model), read-only tools + harness-run end-to-end command | REVIEW.md (strict schema) | Fail if the end-to-end run fails; capped output | Prose judge, no tools | #298 |
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
  an export elsewhere.
- **Outline**, **Edge cases** and **Out of scope.**

If the planner gives no Design, or an invalid one, the harness derives one from
Files allowed and the related exports, and records the rejection reason. The
coder re-validates the Design against its worktree and refuses before the first
write if it no longer holds. A hand-written TASK.md without a Design is still
accepted. The reviewer reports drift from the Design. Docs-only slices have no
Design.

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

## Self-review

After red/green, when the diff changes product code, `src/runtime/self-review.mjs`
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
