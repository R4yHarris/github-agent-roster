# github-agent-roster

A chat-driven Agile software-delivery harness on Node 20 ESM. Roster owns
planning, task files, seat execution, tools, skills, memory, and Git worktrees.
GitHub Issues and PRs are the [board and forge](docs/BOARD.md). Roster is not
a thin CLI wrapper, Hermes Kanban, a Git host, or a separate Kanban database;
it does not replace Git.

## Delivery lifecycle

1. **Ask:** a human states an ask in the `roster` shell or a GitHub issue.
2. **Plan:** the planner breaks the ask into a RECIPE and TASK.
3. **Assign:** the intended planner, coder, and reviewer seats run
   sequentially in one process. Today planner and coder are builtin; the
   reviewer is the human PR reviewer.
4. **Infer:** each model-backed seat calls the **vLLM OpenAI API on DGX Spark**
   first. Today this is the configured planner and coder. Hosted APIs are a
   later, explicit profile using the same HTTP shape. With no endpoint, the
   deterministic stub does not edit code or run tests.
5. **Code:** the coder uses worktree-scoped tools, skills, and recent memory,
   then runs `node --test` after its last edit unless the task explicitly waives tests.
6. **Publish:** reviewed code goes through the required
   [github-agent-contracts](vendor/github-agent-contracts) Git submodule for
   GitHub App identity, human-owned policy, and `AI-Run` trailers.
7. **Evaluate:** a human reviews the PR and posts `AI-Eval:`. Locally recorded
   decisions inform `roster stats` and opt-in `roster recommend` for later
   assignments; PR comments are not automatically imported.

This is a single-process, sequential loop, not a multi-node DGX deployment
or GUI. No Hermes, Claude Code, or Copilot worker is required.

## Status / what runs today

The harness is a [delivery feedback loop](docs/FEEDBACK_LOOP.md), not a chat UI:
[principals](docs/PRINCIPALS.md) bound each seat, [estimates](docs/ESTIMATION.md)
precede work, and tools, research, tests, and excellence checks produce delivery
evidence. Human [retrospectives](docs/RETRO.md) record difficulty, actual minutes,
and verdict; [learning](docs/LEARNING.md) informs the [next task](docs/NEXT.md)
with model capacity and redacted prior feedback. Zero defects is the target,
not a claim inferred from passing tests.

The builtin planner and coder run in one issue worktree when a model endpoint
and model are configured. The [single SWE seat stack](docs/SEAT.md) runs
**principal -> context pack -> research -> skills -> tool loop -> memory ->
excellence -> RESULT.md**. An empty endpoint writes inspectable context,
research, and a stub result, but never edits application code or runs tests.
The shell starts in a TTY. For the
`roster` bin, see [installation](docs/INSTALL.md); these are user commands,
not paths to the CLI source:

```sh
roster
roster --help
roster doctor
roster init
roster ask "Add a Status section to README.md"
roster run --ask-file templates/sdlc/ASK.md --runtime builtin
roster run --issue 42
roster run --issue 42 --auto-model
roster run --issue 42 --publish
roster run --seat coder --runtime builtin
roster prepare --issue 42
roster status --issue 42 --offline
roster recipe validate recipe.yml
roster stats --ref HEAD --evals evals.jsonl
roster eval SESSION accept 3 n
roster recommend --task-class docs
roster vault list
npm test
```

`roster ask` creates a GitHub issue when `gh` is available, or a local draft
and printable issue command when it is not. `roster run --issue N` runs
planner then coder by default; `roster prepare --issue N` preserves the
manual handoff without executing seats. `--runtime builtin` remains accepted
for agents and CI. The `--seats planner,coder` selection is optional;
without `--issue`, `--seat coder --runtime builtin` instead executes the
existing TASK.md in the current worktree, without replanning or publishing.
`--publish` explicitly requests
App publication after a model-backed run and a passing excellence gate. `--auto-model`
requires an empty configured model and at least three matching local human
evaluations or keeps the stub. `roster status --issue N` queries GitHub;
`--offline` uses cached worktree data only. The [offline demo](docs/DEMO.md)
needs neither GitHub nor a model. See the [shell guide](docs/REPL.md) for
`/model`, `/effort`, and `/publish`.

`roster stats` combines contracts `AI-Run` history with opt-in local runs and
local `roster eval` decisions. `roster recommend` does not route automatically
or fetch evaluations from GitHub. `roster vault set NAME` reads piped stdin;
`roster vault get NAME` writes a value only to redirected stdout. There are no
concurrent workers or automatic deploys.

## Setup

`vault set NAME` reads a secret from stdin; `vault list` prints names only,
and `vault get NAME` refuses interactive stdout.
The vault stays under your home directory, never in a Git worktree.

Use Node 20+ and clone with the required contracts submodule:

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
```

For an existing checkout, run `git submodule update --init --recursive`.
The submodule at [`vendor/github-agent-contracts`](vendor/github-agent-contracts)
is pinned to `v0.2.0`; do not copy or rewrite its source.
Run `roster doctor` from the target repository root to check Node 20,
the contracts publisher, App variable presence, policy, and trailer workflow.
It is offline, prints no App values or key paths, and exits nonzero when
prerequisites are missing.
`roster init` copies a config example and `ROSTER-POLICY-NOTE.md` into the
current directory without replacing existing files or the human-owned
`agent-policy.yml`.
See [installation and bootstrap](docs/INSTALL.md) for persistent checkouts,
local npm-exec usage, private config, and the preflight checks.

[Contracts resolution](src/lib/paths.mjs) checks the submodule first, then
`GITHUB_AGENT_CONTRACTS`, then the sibling clone at `../github-agent-contracts`.
It fails if no candidate contains the `scripts/agent-pr.mjs` file. See
[the dependency guide](docs/DEPENDENCY.md) for path semantics and initialization
instructions.

Run tests with `npm test`; there are no runtime package dependencies.
Tests run with no API key or model endpoint.

Copy [the example config](roster.config.example.yml) to ignored
`.roster/config.yml`. The first named profile, `vllm-local`, uses the
**vLLM OpenAI API on DGX Spark** at `http://127.0.0.1:8000/v1`: set
`llm.profile: vllm-local`, leave `llm.base_url: ""`, and choose the served
HF handle for `llm.model`. The profile permits a keyless local server;
other profiles are documented under [endpoints](docs/ENDPOINTS.md). The
tracked example keeps the profile empty so the default is the offline
stub; never put keys in it.

Both issue commands need Git and authenticated `gh` access to an existing
issue on the current repository's GitHub origin. The default run creates
`.worktrees/issue-N`, writes `ASSIGNMENT.md`, `RECIPE.yml`, and `TASK.md`, runs
the coder, and prints a publishing command. The explicit `prepare` command
writes only a manual assignment and ignored `.env`; load that environment into
the worker before publishing. See [same-session seats](docs/MULTIAGENT.md).

Create `.roster/runs` at the repository root to opt into successful-run JSONL
recording. `stats` joins local Git history through the resolved contracts pack
with local runs and `.roster/evals.jsonl`; `--ref` and `--evals` remain supported.
Human `eval` appends a decision, never the coder path. `recommend` suggests the
highest accept-rate only with at least three evaluated samples, otherwise
printing `insufficient data`. Nothing fetches human evaluations from GitHub.

See [the one-task loop](docs/ONE_TASK_LOOP.md), [same-session seats](docs/MULTIAGENT.md),
[the human shell](docs/REPL.md), [recipes](docs/SEATS.md), and
[metrics](docs/METRICS.md), [learning](docs/LEARNING.md),
[model routing](docs/ROUTING.md),
[LLM configuration and the vault](docs/LLM.md), [builtin tools](docs/TOOLS.md),
[the GitHub board](docs/BOARD.md),
[installation](docs/INSTALL.md),
[SDLC](docs/SDLC.md), and
[principals](docs/PRINCIPALS.md) for details.

## Publish

With an LLM configured, `--publish` additionally requires `GITHUB_APP_ID` and
`GITHUB_APP_PRIVATE_KEY_PATH`. After a successful test run (or an explicit task
test waiver) and a passing excellence gate it stages only
task-allowed changes (never policy, workflows, secrets, or generated task
files), then invokes the SDK from the **issue worktree root**. Without
`--publish`, review the diff and publish manually from that same root:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "..." --merge-when-green
```

Initialize the submodule in the worktree first if needed.
The publisher requires a feature branch and a human-owned root
`agent-policy.yml` authorizing coder publication and explicit SDK merging.
It waits for required checks and repository review rules before merging; neither
a recipe nor this CLI grants policy capabilities or deploy rights.
For an issue run, the PR links `Closes #N`. The builtin `--publish` and REPL
`/publish` paths also post an App-authored issue comment after a confirmed
merge (with the coder AI-Run when present) and close the issue if needed.

Never commit credentials or `.env`, or publish as the signed-in human when App
env is set.

The interactive shell's `/publish` imports the same SDK directly rather than
starting another Node process; see [the shell guide](docs/REPL.md).

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
