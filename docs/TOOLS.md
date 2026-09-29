# Builtin coder tools

The configured builtin coder can use only the function tools listed in
`seat.tools` in the [Roster config](../roster.config.example.yml). The builtin
planner has no model-invokable tools, and the offline stub never calls tools
or runs tests. The [tool implementation](../src/runtime/tools.mjs) offers
exactly four functions:

| Tool | Input | Result |
| --- | --- | --- |
| `read_file` | `{ "path": "README.md" }` | UTF-8 text from a regular worktree file. |
| `write_file` | `{ "path": "src/app.mjs", "content": "..." }` | Creates or replaces task-allowed UTF-8 text; returns path and byte count. |
| `list_dir` | `{ "path": "src" }` or `{}` for root | Sorted entry names and types, excluding protected entries. |
| `run_test` | `{}` | Runs `node --test` from the worktree root; returns `exit_code`, captured `stdout`, and `stderr`. |

File paths must be relative to the issue worktree. Absolute paths, path
escapes, symlinked components, and paths resolving outside the worktree
fail rather than being followed. `write_file` also requires an exact file
or directory pattern under `## Files allowed` in the generated
[`TASK.md`](../templates/sdlc/TASK.md); a broad pattern never overrides
the denylist.

Writes cannot touch `.env` or `.env.*` anywhere, `*.pem`, Git metadata,
`agent-policy.yml`, `.github/workflows`, or the pinned
`vendor/github-agent-contracts` dependency. Root `ASSIGNMENT.md`,
`RECIPE.yml`, `TASK.md`, and `RESULT.md` are managed files that the coder
cannot rewrite. `list_dir` refuses protected paths and hides their names
when listing a parent. `read_file` denies secrets and Git metadata; it may
read policy or workflow files as task context but cannot edit them.

`run_test` accepts no arbitrary command or shell arguments. It strips the
configured model API key and App/GitHub credentials from the child
environment, applies a 60-second timeout, and captures test output.
A nonzero Node exit is a failed tool result rather than a process crash,
so the coder can use another turn to fix it. A timeout is an explicit
error. Final verification must pass before a configured run reports
success or publishes. Tests themselves execute project code; these
application-level guards are **not** an OS sandbox. See the
[threat model](THREAT_MODEL.md) for deployment boundaries.
