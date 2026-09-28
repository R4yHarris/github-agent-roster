# github-agent-roster

Orchestrator for **teams of coding agents** that publish through [github-agent-contracts](https://github.com/R4yHarris/github-agent-contracts).

Not CrewAI. Not Hermes Kanban. Not a git host.

| Layer | Owner |
| --- | --- |
| Identity, policy, trailers, `agent-pr` | `github-agent-contracts` |
| Roster, recipes, routing, worktrees | this repo |
| Chat / skills / memory | Hermes, Claude Code, Copilot (workers) |
| Board | GitHub Issues + PRs |

Human loop: **ask** → roster runs the team → **eval** (`AI-Eval` comment).

## Status

Scaffold. Implement from `prompts/` in order. Tag v0.1.0 only after Prompt 01 ships a working one-task loop.

## Layout

```
prompts/          Copilot/Hermes implementation prompts
docs/             architecture
AGENTS.md         harness contract
```
