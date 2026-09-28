# AGENTS.md

This repository is **github-agent-roster**: an orchestrator for teams of coding agents.

GitHub is the board and the forge. Identity, policy, trailers, and `agent-pr.mjs` live in **github-agent-contracts**. This repo assigns work. It does not replace git.

## Hard boundaries

- Work only in this repository unless a task says to read the contracts pack as a dependency.
- Do not invent a Kanban database. Issues + PRs are the queue.
- Do not commit PEMs, tokens, or `.env`.
- Do not edit `agent-policy.yml` in a consumer repo; humans own policy.
- Publish code changes from the repository root with `node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "..."` when App env is set.
- Never `git commit` as the signed-in human when App env is set.

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

1. Human states an ask (GitHub issue).
2. Planner writes a recipe (task graph) on the issue.
3. One coder worker gets a worktree + `AI_*` env.
4. Worker publishes via `agent-pr.mjs`.
5. Human posts `AI-Eval:` on the PR.

No multi-agent swarm until that loop is reliable.
