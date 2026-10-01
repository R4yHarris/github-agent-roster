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
| `/run N [--auto-model]` or `/run --issue N [--auto-model]` | Run builtin planner, coder, then read-only reviewer in one issue worktree. The optional flag chooses a registered fleet profile from qualifying human evaluations or starting priors without rewriting the saved default; no eligible profile leaves an unverified stub. |
| `/status [N] [--offline]` | Show an issue, its open branch PR, and worktree path. Defaults to the last run or created issue; offline reads only the cached assignment and reports PR state as unknown. |
| `/eval TARGET accept\|reject\|rework 1-5 y\|n [--minutes N] [--comment "TEXT"]` | Record the [human retrospective](RETRO.md), including actual minutes and local feedback. |
| `/publish [SUBJECT] [--model MODEL] [--skip-review]` | Publish with an unchanged passing REVIEW.md, or explicitly bypass that verdict. After `/run N`, the default subject is `feat: issue N`; a confirmed merge comments on the still-open issue with PR URL and model ID. Otherwise supply a conventional subject and declare the GHCP model with `--model` or `AI_MODEL`. Completed seat metadata wins over the flag. |
| `/stats [REF]` | Summarize contracts and local AI-Run records, optionally at a Git ref. |
| `/recommend feat\|fix\|docs\|test [--difficulty 1-5]` | Print the same read-only fleet choice as auto-model routing, including its evals/prior reason, or show insufficient data and the config default. |
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

`/run N` reuses an existing registered `issue-N` branch/worktree. Prior
generated outputs are archived locally in Git metadata, while app changes and
the assignment/environment are preserved. An existing branch is never passed
to `git worktree add -b`. Malformed planner tool output is repaired once; if it
still cannot be decoded, RECIPE/TASK stubs and a clear error are written, coding
and publication stay disabled, and `/help`, `/quit`, or a later `/run` still work.
When the existing RECIPE/TASK validate for the issue, `/run N` preserves them
and skips planner execution entirely. A complete newly written TASK also ends
planning immediately instead of consuming another turn for confirmation.
Heading case and the `Original Ask`, `Acceptance Checks`/`acceptance_checks`,
and `Allowed Files` aliases are accepted by all task consumers. Allowed Files
must be explicit, and cached recipes must still match the builtin runtime
schema. A matching task-plan YAML receipt is validated and archived, then
normalized to that fixed schema by the harness without another model request.
`planner skipped artifacts valid` confirms the reuse path. Planning artifact
names in Allowed Files remain write-protected and are excluded from coder scope.
Cached Ask matching uses the issue title or first substantive body line after
whitespace/backtick/template normalization. A valid cached handoff makes no
planner/model request; an empty Original Ask never qualifies.

`/run N` streams timestamped seat activity to stderr immediately, independently
of its final summary. The same metadata is appended to the issue repository's
`.roster/runs/roster-N-coder.log`: seat starts, model/endpoint host, HTTP
phase/status/error class, tool names/paths, managed file writes, mode, and elapsed
milliseconds. No prompts, completions, file bodies, keys, or upstream error
messages are logged. `/status N --offline` reads a bounded local tail and shows
the last seat and last complete line; it does not contact GitHub.

`/model MODEL` and `/effort h` validate and atomically replace only those
fields in the private config, keeping the other fields and comments. The
updated values apply to the next `/run` in this shell. With no selected
endpoint or profile, setting a model alone still leaves the stub active.
`/model clear` leaves the model empty. `/run N --auto-model` explicitly
selects a registered fleet endpoint/model in memory, even if a saved
default exists. It prefers at least three qualifying human evaluations,
otherwise starting priors/class hints; the [routing guide](ROUTING.md)
defines the bounds. Without the flag, an empty model with a configured
endpoint is still an explicit error.
Running `/model` or `/effort` without a value shows the current setting.
Do not put API key values in the config; it remains ignored by Git.
When [onboarding](ONBOARDING.md) saved a project private config, the shell
loads it and `/model`/`/effort` update that file rather than the installed
package's settings.

The CLI equivalent is `roster status --issue N [--offline]`. Without an
explicit number, status uses the current `issue-N` branch or a single issue
worktree; multiple candidates require `--issue N`. `--offline` never calls
GitHub and does not turn unknown PR state into `none`.

## Publish

Review the worktree changes before `/publish`. When both `GITHUB_APP_ID` and
`GITHUB_APP_PRIVATE_KEY_PATH` are set, `/publish` prepares task-allowed files
from a successful configured `/run` with a passing reviewer verdict,
initializes that worktree's contracts
submodule, and invokes the SDK's exported `main` in-process with
`--model` and `--merge-when-green`. Without a run in this shell, it operates on the
current feature worktree and requires you to stage reviewed changes first.
It never commits or pushes with human credentials.
The [review gate](REVIEW.md) checks that REVIEW.md and the exact task/result
evidence it reviewed remain unchanged. Without an in-session run, no trusted
report is available: `/publish` fails unless `--skip-review` is explicit.
The flag bypasses only the reviewer verdict, not tests, excellence, or App
policy. A failed review never deletes coder changes.
Private `publish.enabled: false` blocks `/publish` entirely.
`review.required: false` (legacy alias `reviewer.required`) makes the review verdict optional without
disabling the reviewer, tests, excellence, or human-owned policy.

Without App credentials, `/publish` prints the command instead of executing
it:

```powershell
node vendor\github-agent-contracts\scripts\agent-pr.mjs --message "<subject plus Model, Summary, and how-to-test sections>" --model GPT-6.1-Sol --merge-when-green
```

Without a completed seat, publication requires `AI_MODEL` or `/publish --model MODEL`,
uses `github-copilot`, and omits used/out. Configured LLM model/effort/provider
are not Copilot declarations. `AI_EFFORT=x` declares Max; `AI_CONTEXT_MAX`
describes only known capacity. The task is an issue ID, `AI_TASK`, or branch slug,
and the session uses `ghcp-<date-or-pid>`.
An absent or invalid ID reports `set model` without invoking the publisher
or printing a model-free command. A completed run keeps the actual coder
model/usage rather than a later `/model` setting or explicit publication flag. GHCP sessions use
`AI_MODEL=GPT-6.1-Sol`; see the [full SDK example](GHCP.md).
Generated PR bodies include `## Model`, `## Summary`, and `node --test`
instructions (or an explicit task test waiver); GHCP bodies state that used/out
are `-`. Printed manual commands include metadata assignments and empty values
to clear stale counts. After a run, the summary
comes from the reviewed result; without a run, the conventional subject
describes the staged changes. Issue-run bodies also list the three seats and
disclose a bypass.

Setting only one App variable is an error. An HTTP 422 response from
`--merge-when-green` stops the shell and reports that Checks permission is
not accepted on the installation; there is no human-credential fallback.
The SDK may report that a PR merged but local cleanup failed when the default
branch is checked out in another worktree. Inspect the PR and worktrees in
that case; do not blindly retry publication or create a second commit.
For an issue run, the PR body includes non-closing `Refs #N`. Only after the
SDK reports a merge does Roster verify the PR and open issue, comment with
its URL and real model ID (plus the coder AI-Run if present), and leave the
issue open for the human to close after AI-Eval. This requires App
Issues write and Pull requests read permissions; a failed issue operation
is reported rather than retried with human credentials.
The separate agent/CI `--publish` flag also requests merge-when-green; unlike
the REPL, it invokes the SDK through the builtin runner.

See [same-session seats](MULTIAGENT.md), [the contracts dependency](DEPENDENCY.md),
and [the SDLC](SDLC.md).
