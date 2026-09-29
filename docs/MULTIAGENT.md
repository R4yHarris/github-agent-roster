# Same-session builtin seats

One human GitHub issue is the Ask. One invocation runs two builtin seats in
sequence, in the same process and issue worktree:

```sh
node src/cli.mjs run --issue 42 --runtime builtin
node src/cli.mjs run --issue 42 --runtime builtin --seats planner,coder
```

`--seats planner,coder` is the only supported selection and is the default.
The older `--seat coder` flag is accepted as an alias for that pair. Bare
`run --issue N` remains a prepare-only handoff, not the two-seat executor.
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
3. The coder session `roster-N-coder` reads that `TASK.md`, the worktree's
   `AGENTS.md`, this roster's skills, and recent memory. Its chat loop is
   bounded by `seat.turn_budget`; writes follow the task's allowed paths
   and cannot rewrite `RECIPE.yml` or `TASK.md`. After the configured LLM
   finishes, `node --test` must pass and the plan artifacts must still
   match before optional publication. With no endpoint, the coder writes
   only a stub `RESULT.md`: it does not edit app code or run tests.
4. The command prints one AI-Run line per seat, using its session and only
   its own known usage. Stub lines say `builtin-stub`, without invented LLM
   counts. When `.roster/runs` exists in the issue repository, the two
   completed seats are recorded there, not as a third preparation run.
   A single commit published through the contracts SDK can carry only the
   coder AI-Run trailer; the planner run remains in stdout/local records.

After the coder, the default command **stops** and prints the reviewed
`agent-pr.mjs` command for the issue worktree root. `--publish` retains the
explicit opt-in for a configured LLM run with passing tests and App credentials;
it stages only task-allowed code and delegates a draft PR to the pinned
contracts SDK. Neither path merges or deploys. Human review and `AI-Eval:`
remain outside the coder seat. See [SDLC](SDLC.md), [recipes](SEATS.md),
and [metrics](METRICS.md).
