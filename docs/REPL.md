# Interactive Roster shell

Run `roster` or `node src/cli.mjs` with no arguments from a Git worktree and
an interactive terminal. The banner names the repository, `runtime builtin`,
and the configured `llm.base_url` (or `stub`); the prompt is `roster> `.
`--help` still prints the existing CLI usage and exits 0; empty arguments
with non-TTY stdin print that usage and exit 2. Flags remain available for
agents and CI.

| Command | Behavior |
| --- | --- |
| `/ask TEXT` | Create an issue through `gh`, or write a local Ask, recipe, task, and create command when `gh` is missing. |
| `/run N` or `/run --issue N` | Run the builtin planner then coder in one issue worktree. The stub writes a task and result, but does not edit code or test. |
| `/status` | Show the repository, configured runtime/model endpoint, and the last run in this shell. |
| `/eval TARGET accept|reject|rework 1-5 y|n` | Record a human evaluation through the existing evaluation library. |
| `/publish [SUBJECT]` | Publish reviewed changes using the pinned contracts SDK. After `/run N`, the default subject is `feat: issue N`; otherwise supply a conventional subject. |
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
The separate agent/CI `--publish` flag also requests merge-when-green; unlike
the REPL, it invokes the SDK through the builtin runner.

See [same-session seats](MULTIAGENT.md), [the contracts dependency](DEPENDENCY.md),
and [the SDLC](SDLC.md).
