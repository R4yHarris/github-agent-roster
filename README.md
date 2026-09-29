# github-agent-roster

A standalone Node 20 ESM orchestrator for software tasks. Its **builtin coder
seat** can plan an issue, edit a worktree with an OpenAI-compatible model, run
tests, and prepare a PR. No Hermes, Claude Code, or Copilot worker is required.
[github-agent-contracts](https://github.com/R4yHarris/github-agent-contracts)
is the GitHub **publish SDK only**: it owns identity, policy, trailers, and
`agent-pr.mjs`, not planning or coding. GitHub Issues and PRs remain the board
and forge; there is no Kanban database.

| Layer | Owner |
| --- | --- |
| Identity, policy, trailers, PR publication | `github-agent-contracts` |
| Planner, coder loop, tools, skills, memory, worktrees | this repo |
| Board | GitHub Issues + PRs |

First loop: **issue** → one coder seat → tests → optional App-published draft
PR → human **eval** (`AI-Eval` comment). The empty-URL configuration provides
an offline deterministic stub for setup and tests, not a code-writing worker.

## Status

The builtin coder seat adds SDLC task files and an opt-in LLM execution path to
the original one-task loop. Local learning adds opt-in run records, human
evaluations, and evidence-based recommendations. Do not tag v0.1.0 until the
loop is reviewed and working in a published PR.

## CLI

A separate local-first OpenAI-compatible chat hook and encrypted file vault
are also available; see [LLM configuration and the vault](docs/LLM.md).
Bare `run` remains prepare-only; the builtin runtime is the opt-in LLM path.

```sh
node src/cli.mjs --help
node src/cli.mjs ask "Add a Status section to README.md"
node src/cli.mjs run --issue 42
node src/cli.mjs run --issue 42 --seat coder --runtime builtin
node src/cli.mjs recipe validate recipe.yml
node src/cli.mjs stats --ref HEAD --evals evals.jsonl
node src/cli.mjs vault list
node src/cli.mjs eval roster-20260928T120000000Z accept 3 n
node src/cli.mjs recommend --task-class feat
node --test
```

Copy [the example config](roster.config.example.yml) to ignored
`.roster/config.yml` to set `llm.base_url`, `llm.model`, and optionally the name
of an API-key environment variable. An empty `base_url` uses the stub planner
and writes a task summary to `RESULT.md` **without changing code or running
tests**. `ask` creates a local draft only; it does not create a GitHub issue.
To execute a task, provide an existing issue on the current repository's GitHub
origin to the builtin `run` command. It creates `.worktrees/issue-N`, writes
`ASSIGNMENT.md`, `RECIPE.yml`, and `TASK.md`, runs the coder, and prints a
publishing command. It never publishes without `--publish`.

Bare `run --issue N` retains the earlier prepare-only behavior. Both run modes
need Git and authenticated `gh` access. Create `.roster/runs` at the repository
root to opt into successful-run JSONL recording. `stats` joins local Git
history through the contracts dependency with local runs and
`.roster/evals.jsonl`; `--ref` and `--evals` remain supported. Human `eval`
appends a decision, never the coder path. `recommend` suggests the highest
accept-rate only with at least three evaluated samples, otherwise printing
`insufficient data`. Nothing fetches human evaluations from GitHub.

The human-owned `agent-policy.yml` must authorize coder publication; the
recipe and CLI grant neither merge nor deploy rights. See [SDLC](docs/SDLC.md),
[the one-task loop](docs/ONE_TASK_LOOP.md), [seats](docs/SEATS.md),
[metrics](docs/METRICS.md), and [learning](docs/LEARNING.md).

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
Tests run with no API key or model endpoint.

## Publish

With an LLM configured, `--publish` additionally requires `GITHUB_APP_ID` and
`GITHUB_APP_PRIVATE_KEY_PATH`. After a successful test run it stages only
task-allowed changes (never policy, workflows, secrets, or generated task
files), then invokes the SDK from the **issue worktree root**. Without
`--publish`, review the diff and publish manually from that same root:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "..."
```

Initialize the submodule in the worktree first if needed. Never commit
credentials or publish as the signed-in human when App env is set.

## Layout

```
prompts/          implementation prompts
src/              CLI, planner, builtin runtime, and metrics
skills/           coder instructions loaded from this repo
templates/sdlc/   ask, recipe, task, and assignment templates
tests/            Node test suite and fixtures
docs/             architecture, SDLC, seats, run loop, metrics
AGENTS.md         harness contract
vendor/github-agent-contracts/  required contracts submodule (v0.2.0)
```
