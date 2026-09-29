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

Prompts 00-03 implement the Node 20 ESM scaffold, one-task loop, strict recipes, and local metrics ingest. Local learning adds opt-in run records, human evaluations, and evidence-based recommendations. Do not tag v0.1.0 until the one-task loop is reviewed and working in a published PR.

## CLI

A local-first OpenAI-compatible chat hook and encrypted file vault are also
available. The run loop still makes no LLM calls; see [LLM configuration and
the vault](docs/LLM.md) for the opt-in hook.

```sh
node src/cli.mjs --help
node src/cli.mjs run --issue 42
node src/cli.mjs recipe validate recipe.yml
node src/cli.mjs stats --ref HEAD --evals evals.jsonl
node src/cli.mjs vault list
node src/cli.mjs eval roster-20260928T120000000Z accept 3 n
node src/cli.mjs recommend --task-class feat
node --test
```

`run` needs Git and authenticated `gh` access to an issue on the GitHub origin.
Create `.roster/runs` at the repository root to opt into successful-run JSONL
recording. `stats` joins local Git history through the required contracts pack
with local runs and `.roster/evals.jsonl`; the existing `--ref` and `--evals`
flags remain supported. Human `eval` appends a decision, never the coder path.
`recommend` suggests the highest accept-rate only with at least three evaluated
samples, otherwise printing `insufficient data`. Nothing fetches human
evaluations from GitHub. The repository's human-owned `agent-policy.yml` must
authorize coder publication before the worker uses `agent-pr.mjs`; neither the
recipe nor this CLI grants merge or deploy rights. See [the one-task loop](docs/ONE_TASK_LOOP.md),
[seats](docs/SEATS.md), [metrics](docs/METRICS.md), and [learning](docs/LEARNING.md).

## Setup

`vault set NAME` reads a secret from stdin; `vault list` prints names only.
The vault stays under your home directory, never in a Git worktree.

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
