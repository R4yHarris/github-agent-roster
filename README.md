# github-agent-roster

A standalone Node 20 ESM coding-agent orchestrator for software tasks.
Roster is the team control plane: its builtin planner and coder seats run in
sequence in one process and issue worktree. The planner writes a recipe and
task; the coder can edit code with an OpenAI-compatible model, run tests, and
prepare a PR. No Hermes, Claude Code, or Copilot worker is required. GitHub
Issues and PRs remain the board and forge.

[github-agent-contracts](https://github.com/R4yHarris/github-agent-contracts)
is the **GitHub publish SDK**, consumed as a required
[Git submodule](vendor/github-agent-contracts). It owns App identity, policy
checks, attribution trailers, and `agent-pr.mjs`; it is not the orchestrator.

| Responsibility | Owner |
| --- | --- |
| Team control plane: configuration, planner, coder loop, tools, skills, memory, worktrees | Roster |
| Model inference | Explicitly configured OpenAI-compatible endpoint |
| Optional secret storage for the standalone chat hook | Local encrypted file vault |
| GitHub publication, App identity, policy enforcement, trailers | Contracts submodule |
| Durable queue, review, human evaluation | GitHub Issues + PRs |
| Policy, merge, deploy decisions | Humans and repository protections |

There is no separate Kanban database, and Roster does not replace Git.

## MVP path

```text
config -> explicit /v1 endpoint -> planner -> coder -> tests -> agent-pr -> AI-Eval
```

1. **Config:** select the model, endpoint, and per-seat turn budgets. The CLI uses
   the current repository's GitHub origin and the supplied issue number.
   Keep secrets out of committed configuration.
2. **Local or hosted `/v1` endpoint:** explicitly connect the coder to an
   OpenAI-compatible model, with any required key supplied through the configured
   environment variable. No hosted service is selected by default.
3. **Two sequential seats:** the planner writes the recipe and task in an
   isolated Git worktree; the coder reads them with skills and bounded memory.
   The LLM coder must pass `node --test` after its last edit.
4. **`agent-pr`:** optionally publish reviewed changes through the contracts SDK
   under the GitHub App identity, subject to human-owned policy.
5. **`AI-Eval`:** a human reviews the PR and posts an `AI-Eval:` comment.

Configuration loading and builtin execution are implemented as an opt-in path.
An empty endpoint uses an offline deterministic stub that writes a `RESULT.md`
summary **without editing code or running tests**; it is not a code-writing
worker. A separate vault-aware [chat hook](docs/LLM.md) is available, but its
vault lookup is not wired into the builtin client.

## Available today

The Node 20 ESM CLI supports both the builtin loop and a manual handoff:

- With no arguments in a TTY, `roster` opens the [interactive human shell](docs/REPL.md)
  with slash commands; non-TTY usage and `--help` keep the standard help text.
- `ask "..."` drafts a local ask, recipe, and task; it does not create an issue.
- `run --issue N --runtime builtin` runs builtin planner then coder in one
  worktree. `--seats planner,coder` is optional; publishing requires `--publish`.
- `run --issue N` reads a GitHub issue, creates one coder worktree, writes the
  assignment and ignored `.env`, and prints the next publishing command. It
  does not launch Hermes or any other worker.
- `recipe validate PATH` validates strict seat YAML. Worker labels are
  descriptive; validation neither executes a recipe nor grants permissions.
- `stats` joins contracts `AI-Run` history with opt-in local runs and human
  evaluations. `eval` records a human decision; `recommend` needs at least
  three evaluated samples. Neither fetches evaluations from GitHub.
- `vault set NAME` reads stdin and `vault list` prints names only. A local-first
  OpenAI-compatible chat hook is available independently of the builtin loop.

No concurrent workers, automatic routing, or deploy are provided. The human
shell's `/publish` explicitly requests App merge after green checks.
Do not tag v0.1.0 until the one-task loop is reviewed and working in a published PR.

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

[Contracts resolution](src/lib/paths.mjs) checks the submodule first, then
`GITHUB_AGENT_CONTRACTS`, then the sibling clone at `../github-agent-contracts`.
It fails if no candidate contains the `scripts/agent-pr.mjs` file. See
[the dependency guide](docs/DEPENDENCY.md) for path semantics and initialization
instructions.

Run tests with `npm test`; there are no runtime package dependencies.
Tests run with no API key or model endpoint.

## Current CLI

```sh
node src/cli.mjs
node src/cli.mjs --help
node src/cli.mjs ask "Add a Status section to README.md"
node src/cli.mjs run --issue 42
node src/cli.mjs run --issue 42 --runtime builtin
node src/cli.mjs recipe validate recipe.yml
node src/cli.mjs stats --ref HEAD --evals evals.jsonl
node src/cli.mjs vault list
node src/cli.mjs eval roster-20260928T120000000Z accept 3 n
node src/cli.mjs recommend --task-class feat
npm test
```

Copy [the example config](roster.config.example.yml) to ignored
`.roster/config.yml` to set `llm.base_url`, `llm.model`, and the API-key
environment variable's name. See [endpoint profiles](docs/ENDPOINTS.md).

Both run modes need Git and authenticated `gh` access to an existing issue on
the current repository's GitHub origin. The builtin path creates
`.worktrees/issue-N`, writes `ASSIGNMENT.md`, `RECIPE.yml`, and `TASK.md`, runs
the coder, and prints a publishing command. Bare `run --issue N` remains
prepare-only. For a manual handoff, load the generated ignored `.env` into the
worker environment before publishing. See [same-session seats](docs/MULTIAGENT.md).

Create `.roster/runs` at the repository root to opt into successful-run JSONL
recording. `stats` joins local Git history through the resolved contracts pack
with local runs and `.roster/evals.jsonl`; `--ref` and `--evals` remain supported.
Human `eval` appends a decision, never the coder path. `recommend` suggests the
highest accept-rate only with at least three evaluated samples, otherwise
printing `insufficient data`. Nothing fetches human evaluations from GitHub.

See [the one-task loop](docs/ONE_TASK_LOOP.md), [same-session seats](docs/MULTIAGENT.md),
[the human shell](docs/REPL.md), [recipes](docs/SEATS.md), and
[metrics](docs/METRICS.md), [learning](docs/LEARNING.md),
[LLM configuration and the vault](docs/LLM.md), [SDLC](docs/SDLC.md), and
[principals](docs/PRINCIPALS.md) for details.

## Publish

With an LLM configured, `--publish` additionally requires `GITHUB_APP_ID` and
`GITHUB_APP_PRIVATE_KEY_PATH`. After a successful test run it stages only
task-allowed changes (never policy, workflows, secrets, or generated task
files), then invokes the SDK from the **issue worktree root**. Without
`--publish`, review the diff and publish manually from that same root:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "..."
```

Initialize the submodule in the worktree first if needed.
The publisher requires a feature branch and a human-owned root
`agent-policy.yml` authorizing coder publication. Neither a recipe nor this CLI
grants merge or deploy rights; do not change consumer policy to bypass a denial.

Never commit credentials or `.env`, or publish as the signed-in human when App
env is set.

The interactive shell's `/publish` command instead imports the SDK directly
with `--merge-when-green`; see [the shell guide](docs/REPL.md).

## Layout

```
prompts/          implementation prompts
src/              CLI, planner, builtin runtime, and metrics
skills/           coder instructions loaded from this repo
templates/sdlc/   renderer templates and expanded manual handoff forms
tests/            Node test suite and fixtures
docs/             architecture, SDLC, seats, run loop, metrics
AGENTS.md         harness contract
vendor/github-agent-contracts/  required contracts submodule (v0.2.0)
```
