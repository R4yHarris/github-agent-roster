# AGENTS.md

This repository is **github-agent-roster**: an orchestrator for teams of coding agents.

GitHub is the board and the forge. Identity, policy, trailers, and `agent-pr.mjs` live in **github-agent-contracts**. This repo assigns work. It does not replace git.

## Hard boundaries

- Work only in this repository unless a task says to read the contracts pack as a dependency.
- Do not invent a Kanban database. Issues + PRs are the queue.
- Do not commit PEMs, tokens, or `.env`.
- Do not edit `agent-policy.yml` in a consumer repo; humans own policy.
- Publish code changes with `../github-agent-contracts/scripts/agent-pr.mjs` when App env is set.
- Never `git commit` as the signed-in human when App env is set.

## First loop (v0)

1. Human states an ask (GitHub issue).
2. Planner writes a recipe (task graph) on the issue.
3. One coder worker gets a worktree + `AI_*` env.
4. Worker publishes via `agent-pr.mjs`.
5. Human posts `AI-Eval:` on the PR.

No multi-agent swarm until that loop is reliable.
