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

Prompts 00-03 implement the Node 20 ESM scaffold, one-task loop, strict recipes, and local metrics ingest. Do not tag v0.1.0 until the one-task loop is reviewed and working in a published PR.

## CLI

```sh
node src/cli.mjs --help
node src/cli.mjs run --issue 42
node src/cli.mjs recipe validate recipe.yml
node src/cli.mjs stats --ref HEAD --evals evals.jsonl
node --test
```

`run` needs Git and authenticated `gh` access to an issue on the GitHub origin. `stats` reads local Git history through the sibling `github-agent-contracts` clone; set `GITHUB_AGENT_CONTRACTS` to override its location. `--evals` is optional and reads a local JSONL file; nothing fetches human evaluations from GitHub. The repository's human-owned `agent-policy.yml` must authorize coder publication before the worker uses `agent-pr.mjs`; neither the recipe nor this CLI grants merge or deploy rights. See [the one-task loop](docs/ONE_TASK_LOOP.md), [seats](docs/SEATS.md), and [metrics](docs/METRICS.md) for details.

## Setup

Use Node 20+ and clone with the required contracts submodule:

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
```

For an existing checkout, run `git submodule update --init --recursive`.
The submodule at [`vendor/github-agent-contracts`](vendor/github-agent-contracts)
is pinned to `v0.2.0`; do not copy or rewrite its source.

Contracts resolution checks the submodule first, then `GITHUB_AGENT_CONTRACTS`,
then the sibling clone at `../github-agent-contracts`. It fails if no candidate
contains `scripts/agent-pr.mjs`. See [the dependency guide](docs/DEPENDENCY.md)
for path semantics and initialization instructions.

Run tests with `node --test`; there are no runtime package dependencies.

## Publish

Set `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` in the environment, then
publish from the repository root:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "..."
```

Never commit credentials or publish as the signed-in human when App env is set.

## Layout

```
prompts/          Copilot/Hermes implementation prompts
src/              CLI and prompt implementations
tests/            Node test suite and JSONL fixtures
docs/             architecture, seats, run loop, metrics
AGENTS.md         harness contract
vendor/github-agent-contracts/  required contracts submodule (v0.2.0)
```
