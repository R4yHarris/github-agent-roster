# Same-session builtin seats

One human GitHub issue is the Ask. One invocation runs two builtin seats in
sequence, in the same process and issue worktree:

```sh
roster run --issue 42
roster run --issue 42 --seats planner,coder
```

`--seats planner,coder` is the only supported selection and is the default.
With `--issue N`, the older `--seat coder` flag is accepted as an alias for
that pair. Without an issue, `roster run --seat coder --runtime builtin`
runs the [single coder stack](SEAT.md) on an existing TASK.md instead.
`--runtime builtin` remains available for CI and GHCP. Use
`roster prepare --issue N` for a handoff without executing the seats.
This is not a concurrent swarm, two Copilot chats, or a Hermes Kanban loop.
GitHub Issues and PRs are the board; no additional task database or issue
is created.

1. The issue lookup creates `.worktrees/issue-N` once, with `ASSIGNMENT.md`
   and an ignored `.env` holding `AI_TASK=issue-N` and
   `AI_SESSION=roster-N-coder`. Both seats use that worktree.
2. The planner session `roster-N-planner` uses the deterministic stub when
   `llm.base_url` is empty. Otherwise it requests a strict JSON plan from
   the configured model. It can retry invalid plans up to
   `planner.turn_budget` (1-64) but has **no tools**: a `write_file` request,
   including one targeting `src/`, fails instead of running. The planner
   renders the validated [SDLC templates](../templates/sdlc/) as root
   `RECIPE.yml` and `TASK.md`. One issue produces one task in this loop.
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
4. The command prints one AI-Run line per configured LLM seat, using its session
   and only its own known usage. Stub runs omit AI-Run, model, and LLM counts.
   The two completed seats are recorded automatically in the issue
   repository's ignored `.roster/runs`, not as a third preparation run.
   Coder records include a passing/failing excellence flag; configured failures
   with result evidence are recorded before propagating their error.
   A single commit published through the contracts SDK can carry only the
   coder AI-Run trailer; the planner run remains in stdout/local records.

The append-only seat journals default to `.roster/memory/planner.jsonl` and
`.roster/memory/coder.jsonl` in the roster installation. Each prompt receives
at most the last 20 lines of its own journal as data, not instructions.
`paths.memory` preserves a custom coder filename; the planner journal stays
beside it. These ignored files are not a second task board.

After the coder, the default command **stops** and prints the reviewed
`agent-pr.mjs` command for the issue worktree root. `--publish` retains the
explicit opt-in for a configured LLM run with passing tests and App credentials;
it stages only task-allowed code and delegates a draft PR to the pinned
contracts SDK, which marks the PR ready and merges only after required checks
and repository protections permit it. Neither path deploys. Human review and
`AI-Eval:` remain outside the coder seat. See [SDLC](SDLC.md), [recipes](SEATS.md),
and [metrics](METRICS.md).
