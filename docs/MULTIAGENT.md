# Same-session builtin seats

One human GitHub issue is the Ask. One invocation runs three builtin seats in
sequence, in the same process and issue worktree:

```sh
roster run --issue 42
roster run --issue 42 --seats planner,coder,reviewer
```

`--seats planner,coder,reviewer` is the default and supported selection;
the older `--seats planner,coder` spelling aliases that full sequence.
With `--issue N`, the older `--seat coder` flag is accepted as an alias for
that sequence. Without an issue, `roster run --seat coder --runtime builtin`
runs the [single coder stack](SEAT.md) on an existing TASK.md instead.
`--runtime builtin` remains available for CI and GHCP. Use
`roster prepare --issue N` for a handoff without executing the seats.
The default is not a concurrent swarm, two Copilot chats, or a Hermes Kanban loop.
GitHub Issues and PRs are the board; no additional task database or issue
is created.
`--auto-model` explicitly selects a registered fleet profile for that one
run, using qualifying human evaluations before starting priors. It changes
neither the saved default nor the sequential seat/worktree topology.
See [opt-in routing](ROUTING.md) for the source and context constraints.

## Opt-in parallel child waves

```sh
roster run --issue 42 --parallel 2 --auto-model
```

`--parallel K` (also `/run N --parallel K`) applies to executable feature
children, not to seats within one child. Every child still runs
planner -> coder -> reviewer sequentially in its own `.worktrees/issue-N`
and existing `issue-N` branch. `K` must be a positive safe integer; default
`1` preserves the existing single-child order and return shape.
The task bound is the smaller of K and the sum of registered fleet profile
concurrency when routing is enabled. Saved/default runs use their configured
concurrency, or 1 when undeclared. Per-profile HTTP admission still governs
requests independently; summed capacity does not promise balanced routing.

New child issues contain `Depends on: none` or comma-separated GitHub issue
links (`Depends on: #100, #101`). Same-wave drafts are independent; later
waves link to earlier-wave children. Explicit links govern readiness.
Older issues without that declaration retain their earlier-wave barrier.
Dependencies must be closed on GitHub: passing local review or merging a PR
does not silently close an issue or approve its human evaluation.
After each bounded group completes, the board is refreshed and newly ready
children may run. Each child is attempted at most once in an invocation.
Cycles, open dependencies, claims and review labels remain blocked/not ready;
lookup errors fail closed. There is no persistent scheduler or task database.

Each child has prefixed live output, its own log and shell status-rail row.
An exclusive repository-common claim lock prevents another local invocation
writing that issue, including ordinary single-issue runs. Locks are released
on success/error/cancellation; existing dead-owner recovery applies after a
process crash. GitHub status uses the existing App labels/comments.
One child failure preserves sibling results and later independent children;
the aggregate returns `children`, refreshed `waves`, the effective `parallel`
limit and a failing outcome. Board-refresh failures retain completed evidence.
Cancellation drains in-flight children before returning and starts no new ones.

Confirmation, plan mode and shared coder steering cannot combine with K > 1.
Parallel publication requires explicit `--publish` and retains every child's
test/review/App gates; there is no parent commit or parent PR. Without it,
use each child's printed reviewed App SDK handoff from that child's root.
The shell refuses `/publish` on an aggregate rather than picking a child or
publishing the planning worktree. Merge remains the contracts publisher's
role; this scheduler never merges sibling branches or closes issues.

1. The issue lookup creates `.worktrees/issue-N` once, with `ASSIGNMENT.md`
   and an ignored `.env` holding `AI_TASK=issue-N` and
   `AI_SESSION=roster-N-coder`. All three seats use that worktree.
   Repeating `/run N` reuses its registered worktree and branch. An existing
   branch without a worktree is added without `-b`; mismatched registrations
   are refused. App files, ASSIGNMENT.md, and `.env` are preserved. Previous
   untracked generated run artifacts are archived under the repository's Git
   metadata before another attempt, not deleted or mixed into the app diff.
   If existing RECIPE.yml and TASK.md validate for this issue, planning is
   skipped and the coder starts directly. Those two files stay unchanged;
   estimation and coder/reviewer outputs are regenerated. Failed stubs,
   mismatched Ask/recipe references, and invalid runtime recipes do not bypass
   planning. The required three-seat recipe schema is unchanged.
   A matching model-written task-plan receipt can also be reused: its title,
   checks, and application paths are validated against TASK.md, the receipt is
   archived, and the harness writes the fixed builtin recipe without planner
   HTTP. Logs say `planner skipped artifacts valid`. Listed planner bookkeeping
   files do not become coder write permissions.
2. The planner session `roster-N-planner` uses the deterministic stub when
   `llm.base_url` is empty. Otherwise it requests a strict JSON plan from
   the configured model. Within `planner.turn_budget` (1-64), it may use
   `write_file` for exactly root `RECIPE.yml`, `TASK.md`, and `ESTIMATE.md`.
   App-code writes, including `src/`, are denied and reported as tool errors.
   The model can return a JSON plan or finish after writing a complete
   TASK with the unchanged Ask, acceptance checks, allowed files, and metadata.
   A complete written TASK ends planning immediately, without a second model
   turn merely to confirm it. Headings are case-insensitive: a title heading,
   `Original Ask` or `Ask` containing the issue text, `Acceptance Checks` or
   `acceptance_checks`, and `Allowed Files` or `Files allowed`. Scope and other
   sections may appear between them. Missing Allowed Files is an error; paths
   are never inferred from the prose of a written task.
   Ask matching collapses whitespace, removes wrapping backticks and issue
   template boilerplate, and accepts the issue title or the first substantive
   nonempty body line in that section; it does not require the whole templated
   body to be repeated. `Ask (unchanged)` and `Original Ask (verbatim)` are
   accepted heading annotations. Empty or template-only Ask sections still fail.
   The same check applies to cached files before deciding to skip planning.
   The harness validates that task and finalizes all three managed artifacts
   through the same scoped writer, including trusted history-based estimates.
   Draft recipe/estimate text cannot add seats or replace estimation evidence.
   One issue produces one task in this loop.
   OpenAI `function.arguments` JSON strings are decoded once; a JSON tool
   payload embedded in model text is also accepted. Malformed output gets
   exactly one short `emit only tool_calls` repair, at most one supplemental
   response beyond the normal budget. If repair fails, the planner returns
   a clear error and unverified RECIPE/TASK stubs. No configured coder, tests,
   or publisher runs; the interactive shell remains usable.
   It reads and appends only its own recent planner memory outside the issue
   worktree, not app code. It also reads the issue repository's human evaluation
   history for [next-task model/estimate selection and redacted prior feedback](NEXT.md).
3. The coder session `roster-N-coder` follows the [SWE seat sequence](SEAT.md):
   principal, bounded context, read-only research, task-selected skills, tool
   loop, memory, excellence, then RESULT.md. Its context includes that
   `TASK.md`, the worktree's `AGENTS.md`, redacted prior feedback, and its own
   recent memory. Its chat loop is
   bounded by `seat.turn_budget`; writes follow the task's allowed paths
   and cannot rewrite `RECIPE.yml` or `TASK.md`. After the configured LLM
   finishes, `node --test` must pass and the plan artifacts must still
   match before optional publication. With no endpoint, the coder writes
   context/research artifacts and a stub `RESULT.md`: it does not edit app code
   or run tests. The excellence report stays explicitly unverified.
   `write_file` rejects the root recipe and task even if a model requests
   them. The runner rechecks both files after the coder's final tests and
   refuses publication if either changed.
4. After the coder writes RESULT.md, the reviewer session
   `roster-N-reviewer` reads the acceptance checks, the report, and the
   task-allowed diff. It cannot write: it may only ask the harness for
   read-only `read_file`, `search_text`, and `git_diff` answers, and a
   `write_file` request is refused. The harness also runs the read-only
   `roster` commands the checks name ([verifying reviewer](SWE_LIFECYCLE.md#verifying-reviewer)).
   The harness writes REVIEW.md with pass/fail reasons and security notes.
   A stub or missing evidence fails, preserving the coder's diff. The
   [review gate](REVIEW.md) blocks publication by default on a failed or
   changed report; only explicit `--skip-review` bypasses this verdict.
5. The command prints one AI-Run line per configured LLM seat, using its session
   and only its own known usage. Stub runs omit AI-Run, model, and LLM counts.
   The three completed seats are recorded automatically in the issue
   repository's ignored `.roster/runs`, not as a third preparation run.
   Coder records include a passing/failing excellence flag; configured failures
   with result evidence are recorded before propagating their error.
   A single commit published through the contracts SDK can carry only the
   coder AI-Run trailer; planner and reviewer runs remain in stdout/local records.

The append-only seat journals default to `.roster/memory/planner.jsonl` and
`.roster/memory/coder.jsonl` in the roster installation. Each prompt receives
at most the last 20 lines of its own journal as data, not instructions.
`paths.memory` preserves a custom coder filename; the planner journal stays
beside it. These ignored files are not a second task board.

After the reviewer, the default command **stops** and prints the
`agent-pr.mjs` command for the issue worktree root only when review passes.
`--publish` retains the explicit opt-in for a configured LLM run with passing
tests, review (unless explicitly bypassed), and App credentials;
it stages only task-allowed code and delegates a draft PR to the pinned
contracts SDK, which marks the PR ready and merges only after required checks
and repository protections permit it. Neither path deploys. Human review and
`AI-Eval:` remain outside the coder seat. See [SDLC](SDLC.md), [recipes](SEATS.md),
and [metrics](METRICS.md).
