# Builtin coder tools

The configured builtin coder can use only the function tools listed in
`seat.tools` in the [Roster config](../roster.config.example.yml). The builtin
planner has no model-invokable tools, and the offline stub never calls tools
or runs tests. The [tool implementation](../src/runtime/tools.mjs) offers
exactly five functions:

| Tool | Input | Result |
| --- | --- | --- |
| `read_file` | `{ "path": "README.md" }` | UTF-8 text from a regular worktree file. |
| `write_file` | `{ "path": "src/app.mjs", "content": "..." }` | Creates or replaces task-allowed UTF-8 text; returns path and byte count. |
| `list_dir` | `{ "path": "src" }` or `{}` for root | Sorted entry names and types, excluding protected entries. |
| `run_test` | `{}` | Runs `node --test` from the worktree root; returns `exit_code`, captured `stdout`, and `stderr`. |
| `search_text` | `{ "query": "literal text", "path": "src" }` (`path` optional) | At most 50 `{ path, line, text }` matches and a `truncated` flag. |

File paths must be relative to the issue worktree. Absolute paths, path
escapes, symlinked components, and paths resolving outside the worktree
fail rather than being followed. Ambiguous Windows components (trailing
dots/spaces) and alternate data streams are refused on every platform.
`write_file` also requires an exact file
or directory pattern under `## Files allowed` in the generated
[`TASK.md`](../templates/sdlc/TASK.md); a broad pattern never overrides
the denylist.

All file tools deny `.env`, `.env.*`, and `*.env` anywhere, `*.pem`, vault
storage under `.roster/vault`, Git metadata,
`agent-policy.yml`, `.github/workflows`, or the pinned
`vendor/github-agent-contracts` dependency. Root `ASSIGNMENT.md`,
`RECIPE.yml`, `TASK.md`, `CONTEXT.md`, `ESTIMATE.md`, and `RESULT.md` are managed files that the coder
cannot rewrite. `list_dir` refuses protected paths and hides their names
when listing a parent. Policy and workflow bodies are no longer readable
as task context; both reads and writes are denied.
The human evaluation ledger `.roster/evals.jsonl` is also write-protected,
even when TASK.md grants broad write scope.

`search_text` is a case-sensitive, fixed-string grep, not a regex or a shell
command. It walks regular worktree files in sorted directory order, uses
the same read/list guards, skips binary files containing NUL and directory
symlinks, and never traverses protected entries. `path` can select a file or
subdirectory; an explicitly requested unsafe path fails rather than silently
returning no matches. Line numbers are one-based. An additional match beyond
the 50 returned lines sets `truncated: true`; exactly 50 matches is not
reported as truncated. No subprocess is used for search and no npm dependency
is required.

`run_test` accepts no arbitrary command or shell arguments. It strips the
configured model API key and App/GitHub credentials from the child
environment, marks the child as `ROSTER_SEAT=coder` to preserve the human-only
evaluation boundary, applies a 60-second timeout, and captures test output.
A nonzero Node exit is a failed tool result rather than a process crash,
so the coder can use another turn to fix it. A timeout is an explicit
error. Final verification must pass before a configured run reports
success or publishes. Tests themselves execute project code; these
application-level guards are **not** an OS sandbox. See the
[threat model](THREAT_MODEL.md) for deployment boundaries.
