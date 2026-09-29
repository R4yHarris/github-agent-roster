# AGENTS.md

This repository is **github-agent-roster**: a standalone orchestrator for
software tasks. A builtin planner then a builtin coder run in the same process
and issue worktree; neither Hermes, Claude Code, nor Copilot is required.

GitHub is the board and the forge. This repo owns planning, task files, coder
execution, skills, tools, memory, and worktrees. Identity, policy, trailers,
and `agent-pr.mjs` live in **github-agent-contracts**, the GitHub publish SDK
only. This repo does not replace git.

## Hard boundaries

- Work only in this repository unless a task says to read the contracts pack as a dependency.
- Do not invent a Kanban database. Issues + PRs are the queue.
- Do not commit PEMs, tokens, or `.env`.
- Do not edit `agent-policy.yml` in a consumer repo; humans own policy.
- Publish reviewed code changes from the current feature worktree's repository root with `node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "..." --merge-when-green` when App env is set. Never leave a draft PR for the human.
- Never `git commit` as the signed-in human when App env is set.
- Never use `gh pr create` or `git push` with human credentials for GHCP publication.
- Never edit `.github/workflows/*` from the coder seat.

## Contracts dependency

Use the required [`vendor/github-agent-contracts`](vendor/github-agent-contracts)
Git submodule, pinned to `v0.2.0`. Clone with `git clone --recurse-submodules`,
or initialize an existing checkout with `git submodule update --init --recursive`.
Do not copy contracts source into this tree or rewrite files in the submodule.

Use [`resolveContractsPath`](src/lib/paths.mjs): submodule first, then
`GITHUB_AGENT_CONTRACTS`, then the sibling `../github-agent-contracts`.
Fail if no candidate contains the `scripts/agent-pr.mjs` file. See
[the dependency guide](docs/DEPENDENCY.md).

## First loop (v0)

1. Human states an ask (GitHub issue; a local `roster ask` draft does not create one).
2. Stub or configured LLM planner writes a two-seat RECIPE and one TASK in the issue worktree, without app-code tools.
3. Builtin coder loads AGENTS.md, TASK.md, skills, and recent memory into a separate bounded loop in that worktree.
4. Configured coder runs `node --test`; publication uses `agent-pr.mjs` only on explicit request.
5. Human posts `AI-Eval:` on the PR.

With no LLM endpoint, the deterministic stub writes a RESULT summary and does
not change code or run tests. There is no concurrent swarm or separate task
board. See [same-session seats](docs/MULTIAGENT.md) and [SDLC](docs/SDLC.md).

GHCP subagent names `planner` and `coder` map to these sequential builtin seats
in one run/worktree, not separate chats, a second queue, Hermes Kanban, or a
new runtime. Follow the [GHCP bridge](docs/GHCP.md) for its explicit App
publication command and HTTP 422 handling; the builtin loop is unchanged.
