# AGENTS.md

This repository is **github-agent-roster**: a standalone orchestrator for
software tasks. A builtin planner then a builtin coder run in the same process
and issue worktree; neither Hermes, Claude Code, nor Copilot is required.

GitHub is the board and the forge. This repo owns planning, task files, coder
execution, skills, tools, memory, and worktrees. Identity, policy, trailers,
and `agent-pr.mjs` live in **github-agent-contracts**, the GitHub publish SDK
only. This repo does not replace git.
The GitHub App is the authenticated principal; neither the model, a GHCP
subagent name, nor `AI_*` metadata grants policy capabilities.

## Hard boundaries

- Work only in this repository unless a task says to read the contracts pack as a dependency.
- Do not invent a Kanban database. Issues + PRs are the queue.
- Do not commit PEMs, tokens, or `.env`.
- Do not edit `agent-policy.yml` in a consumer repo; humans own policy.
- Publish reviewed code changes only from the current feature worktree's
  repository root using `node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "..." --merge-when-green`
  with both App credentials set. Never leave a draft PR for the human.
- Never `git commit` as the signed-in human when `GITHUB_APP_ID` is set, even
  if the App private-key path is missing. Stop instead of using human credentials.
- Never use `gh pr create` or `git push` with human credentials for GHCP publication.
- Never edit `.github/workflows/*` from the coder seat; humans own workflows.

## Contracts dependency

Use the required [`vendor/github-agent-contracts`](vendor/github-agent-contracts)
Git submodule, pinned to the fail-closed `v0.2.1`. Clone with `git clone --recurse-submodules`,
or initialize an existing checkout with `git submodule update --init --recursive`.
Do not copy contracts source into this tree or rewrite files in the submodule.

Use [`resolveContractsPath`](src/lib/paths.mjs): submodule first, then
`GITHUB_AGENT_CONTRACTS`, then the sibling `../github-agent-contracts`.
Fail if no candidate contains the `scripts/agent-pr.mjs` file. See
[the dependency guide](docs/DEPENDENCY.md).

## First loop (v0)

1. Human states an ask (`roster ask` creates a GitHub issue when `gh` is available, otherwise an offline draft).
2. Stub or configured LLM planner reads its own recent memory and writes a three-seat RECIPE and one TASK in the issue worktree, without app-code tools.
3. Builtin coder loads AGENTS.md, TASK.md, skills, and its own recent memory into a separate bounded loop in that worktree.
4. Builtin reviewer reads the task checks, coder RESULT.md, and diff without app-code tools, then writes REVIEW.md in the same process. It cannot merge or publish.
5. Configured coder runs `node --test`; Roster-managed publication requires a passing REVIEW.md unless `--skip-review` is explicit. It uses `agent-pr.mjs` only on explicit request, comments with the coder AI-Run, and closes the issue after a confirmed merge.
6. Human posts `AI-Eval:` on the PR.

With no LLM endpoint, the deterministic stub writes a RESULT summary and does
not change code or run tests. There is no concurrent swarm or separate task
board. See [same-session seats](docs/MULTIAGENT.md) and [SDLC](docs/SDLC.md).

GHCP subagent names `planner` and `coder` map to these sequential builtin seats
in one run/worktree; reviewer is a later read-only builtin seat, not another
chat, queue, or runtime. The parent Copilot calls
`node src/cli.mjs run --issue N --runtime builtin --seats planner,coder,reviewer`
for the issue, then the App publisher for reviewed changes. It must not
`git commit` as R4yHarris when `GITHUB_APP_*` is set. Follow the
[GHCP bridge](docs/GHCP.md) for publication and HTTP 422 handling.
