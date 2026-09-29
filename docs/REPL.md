# Interactive Roster shell

Run `roster` with no arguments from a Git worktree and an interactive terminal
after [installing it](INSTALL.md). The banner names the repository, `seat coder`,
`runtime builtin`, and the configured `llm.base_url` (or `stub`); the prompt
is `roster> `.
`--help` still prints the existing CLI usage and exits 0; empty arguments
with non-TTY stdin print that usage and exit 2. Flags remain available for
agents and CI.

| Command | Behavior |
| --- | --- |
| `/ask TEXT` | Create an issue through `gh`, or write a local Ask, recipe, task, and create command when `gh` is missing. |
| `/model [MODEL]` | Show the current model or persist a new one to ignored `.roster/config.yml`. |
| `/effort [l|m|h|x]` | Show the current effort or persist a new level to ignored `.roster/config.yml`. |
| `/run N [--auto-model]` or `/run --issue N [--auto-model]` | Run the builtin planner then coder in one issue worktree. The optional flag routes an empty configured model only with enough human evaluations; otherwise the stub writes a task and result without editing code or testing. |
| `/status [N] [--offline]` | Show an issue, its open branch PR, and worktree path. Defaults to the last run or created issue; offline reads only the cached assignment and reports PR state as unknown. |
| `/eval TARGET accept|reject|rework 1-5 y|n` | Record a human evaluation through the existing evaluation library. |
| `/publish [SUBJECT]` | Publish reviewed changes using the pinned contracts SDK. After `/run N`, the default subject is `feat: issue N`; a confirmed merge comments on and closes that issue. Otherwise supply a conventional subject. |
| `/stats [REF]` | Summarize contracts and local AI-Run records, optionally at a Git ref. |
| `/recommend feat|fix|docs|test` | Recommend from evaluated local runs. |
| `/vault` or `/vault list` | List vault entry names, never values. |
| `/vault get NAME` | Check whether an entry exists without revealing its value; use piped `roster vault get NAME` to retrieve it. |
| `/vault set NAME` | Read the next line with terminal echo and readline history disabled, then store it in the existing file vault. |
| `/help` | Show slash-command help. |
| `/quit` | Exit with status 0; Ctrl+C also exits 0. |

Unknown commands report an error and leave the prompt open. Secrets are never
accepted as part of `/vault set NAME` itself. The shell dispatches to existing
library functions in the same Node process; those libraries may still invoke
Git, GitHub CLI, tests, or the metrics exporter. It does not spawn another
Roster CLI process or add a queue.

`/model MODEL` and `/effort h` validate and atomically replace only those
fields in the private config, keeping the other fields and comments. The
updated values apply to the next `/run` in this shell. With no selected
endpoint or profile, setting a model alone still leaves the stub active.
`/model clear` leaves the model empty; `/run N --auto-model` can then apply
a recommendation in memory only when at least three matching human
evaluations exist. Without that flag, an empty model with a configured
endpoint is an explicit error.
Running `/model` or `/effort` without a value shows the current setting.
Do not put API key values in the config; it remains ignored by Git.

The CLI equivalent is `roster status --issue N [--offline]`. Without an
explicit number, status uses the current `issue-N` branch or a single issue
worktree; multiple candidates require `--issue N`. `--offline` never calls
GitHub and does not turn unknown PR state into `none`.

## Publish

Review the worktree changes before `/publish`. When both `GITHUB_APP_ID` and
`GITHUB_APP_PRIVATE_KEY_PATH` are set, `/publish` prepares task-allowed files
from a successful configured `/run`, initializes that worktree's contracts
submodule, and invokes the SDK's exported `main` in-process with
`--merge-when-green`. Without a run in this shell, it operates on the
current feature worktree and requires you to stage reviewed changes first.
It never commits or pushes with human credentials.

Without App credentials, `/publish` prints the command instead of executing
it:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "<conventional subject>" --merge-when-green
```

Setting only one App variable is an error. An HTTP 422 response from
`--merge-when-green` stops the shell and reports that Checks permission is
not accepted on the installation; there is no human-credential fallback.
The SDK may report that a PR merged but local cleanup failed when the default
branch is checked out in another worktree. Inspect the PR and worktrees in
that case; do not blindly retry publication or create a second commit.
For an issue run, the PR body includes `Closes #N`. Only after the SDK reports
a merge does Roster verify the PR, comment with the coder AI-Run if present,
and close the issue if GitHub has not already done so. This requires App
Issues write and Pull requests read permissions; a failed issue operation
is reported rather than retried with human credentials.
The separate agent/CI `--publish` flag also requests merge-when-green; unlike
the REPL, it invokes the SDK through the builtin runner.

See [same-session seats](MULTIAGENT.md), [the contracts dependency](DEPENDENCY.md),
and [the SDLC](SDLC.md).
