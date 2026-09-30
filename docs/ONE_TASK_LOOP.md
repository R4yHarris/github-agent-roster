# One-task loop

The default builtin path is `roster run --issue N` ([SDLC](SDLC.md)).
`--runtime builtin` remains accepted for agents and CI. A run reads the
existing issue, creates one worktree, runs a
builtin planner to write `RECIPE.yml` and `TASK.md`, then loads roster
context/skills/memory into the bounded coder loop and writes `RESULT.md`.
The same-process, read-only [reviewer](REVIEW.md) then reads the task,
result, and diff and writes REVIEW.md with pass/fail reasons and security notes.
When no LLM endpoint
is configured, the deterministic stub writes only the result summary and
does **not** implement the ask or run tests. With an endpoint, the coder uses
five guarded tools and must pass a final `node --test` run. A failed review
keeps the work but suppresses the publish command. It prints an
`agent-pr.mjs` command with an explicit `--model` and a `Closes #N` message for the issue worktree
root; `--publish` opts into staging task-allowed
changes and invoking the App SDK. Policy, workflows, and credentials are
never staged by the coder seat.
An unchanged passing REVIEW.md is required for Roster-managed publication
unless `--skip-review` explicitly bypasses the reviewer verdict; neither
option waives coder excellence or required checks. A direct contracts SDK
command is outside this gate and requires human review.

Manual handoff remains available explicitly:

`runIssue(issueNumber)` in `src/lib/issue.mjs` implements
`roster prepare --issue N`. It can also be called directly:

```js
import { runIssue } from './lib/issue.mjs';

await runIssue('42');
```

The caller must run it from within the repository containing the GitHub issue.
The live command requires Git, an installed and authenticated GitHub CLI (`gh`),
and an accessible GitHub issue on the **current repository's origin remote**.
`runIssue` finds the repository root with `git rev-parse --show-toplevel`, reads
its GitHub HTTPS or SSH origin, and runs
`gh issue view N --repo OWNER/REPO --json number,title,body,url` there. It does
not read cached issue data or create issues. A missing issue, empty issue body
(the Ask), or failed command is an error; no worktree is created if issue
lookup fails.

For issue `N`, the function creates branch `issue-N` from the current HEAD in
`.worktrees/issue-N`, writes `ASSIGNMENT.md` with the issue URL, number, title,
and unmodified body under **Ask**, and writes an ignored `.env` file:

```dotenv
AI_TASK=issue-N
AI_SESSION=roster-<UTC timestamp>
```

If the repository root already contains `.roster/runs`, successful assignment
setup appends one local JSONL run record. It includes the generated session and
task plus known model, effort, and context metadata; it does not pretend the
starting HEAD is the eventual worker commit. Without that directory, run
recording is disabled. No coder path writes human `AI-Eval` decisions. See
[learning](LEARNING.md) for manual opt-in recording, automatic seat runs,
human evaluations, and recommendations.

It returns the issue, worktree and file paths, task, session, and `nextCommand`.
It prints the worktree location and, when a model is configured, a command
for the worker **after editing inside the worktree and loading `.env` into
its environment**:

```sh
node "$GITHUB_AGENT_CONTRACTS/scripts/agent-pr.mjs" --message "<subject plus Model, Summary, how-to-test, and Closes #N>" --model "$AI_MODEL" --merge-when-green
```

Set `GITHUB_AGENT_CONTRACTS` to the resolved contracts pack's absolute path in
the worker environment, normally the initialized `vendor/github-agent-contracts`
submodule; see [dependency resolution](DEPENDENCY.md). The generated message includes `## Model`, `## Summary`, test instructions,
and `Closes #N`; update its summary to describe the changes actually reviewed.
Model resolution is `config.llm.model`, then `AI_MODEL`, then `ROSTER_MODEL`.
When none is set, preparation still succeeds but returns `nextCommand: null`
and reports `set model`; it does not print a runnable model-free command.
Printed commands use the caller platform's shell quoting and environment
variable syntax, including PowerShell on Windows.
`roster prepare --issue N` only prepares a coder handoff and
prints the manual publishing command: it does not
start a worker, merge, or open additional issues. A later explicit publish
links the issue and requests merge only after the SDK verifies reviewed policy
and required checks. The builtin publish path posts an App-authored comment
with its coder AI-Run and closes the issue only after verifying the merged PR.
The raw handoff command supplies the closing reference but does not run
Roster's post-merge comment hook.
The builtin path reuses this issue lookup and assignment
preparation, rendering `ASSIGNMENT.md` from the SDLC template, but sets the
ignored `.env` session to `roster-N-coder` and records separate planner/coder
sessions. See [same-session seats](MULTIAGENT.md).

Tests inject `runCommand`, `fileSystem`, `env`, `now`, and `log` into `runIssue` to
simulate git, gh, file writes, timestamps, and output without a network call.
Run the focused suite with `node --test tests/issue.test.mjs tests/learn.test.mjs`.
