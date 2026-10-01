# Builtin seat tools

The configured builtin coder can use only the function tools listed in
`seat.tools` in the [Roster config](../roster.config.example.yml). The builtin
planner has only the artifact-scoped writer described below; reviewer has no
model-invokable tools. The offline stub makes no model tool calls or test runs.
The [tool implementation](../src/runtime/tools.mjs) offers the coder exactly
five functions:

| Tool | Input | Result |
| --- | --- | --- |
| `read_file` | `{ "path": "README.md", "max_lines": 200 }` (`max_lines` optional) | UTF-8 text from a regular worktree file, optionally limited to the first positive integer number of lines. |
| `write_file` | `{ "path": "src/app.mjs", "content": "..." }` | Creates or replaces task-allowed UTF-8 text; returns path and byte count. |
| `list_dir` | `{ "path": "src" }` or `{}` for root | Sorted entry names and types, excluding protected entries. |
| `run_test` | `{}` | Runs `node --test` from the worktree root; returns `exit_code`, captured `stdout`, and `stderr`. |
| `search_text` | `{ "query": "literal text", "path": "src" }` (`path` optional) | At most 50 `{ path, line, text }` matches and a `truncated` flag. |

## Planner artifact writer

A configured slice planner in an issue worktree receives only `write_file`.
Its path schema and runtime guard allow exactly `RECIPE.yml`, `TASK.md`,
and `ESTIMATE.md` at that root, not directories, aliases, absolute paths,
app files, or other managed files. Planner scope does not inherit the
coder's `files_allowed` patterns. Each UTF-8 artifact is bounded to 64 KiB;
only drafts created by this writer may be replaced. Pre-existing files,
symlinks, hard links, and externally modified drafts are refused.
Known credentials and private-key material cannot be persisted in drafts.

Feature and initiative planners instead return bounded, validated JSON without
model-invokable tools. Their harness writer permits only root PLAN.md, not
TASK/recipe/estimate, directories, or app code. Features must contain 2-5
distinct child issue drafts; initiatives contain outcomes, waves, and issues.
`wave:N` labels are generated from contiguous positive wave numbers. Optional
child file lists cannot exceed human-named scope, and unknown scope remains a
human clarification, not a wildcard. One invalid JSON/shape correction is
allowed within the planner budget; failure is explicit and never starts coder.
The same regular-file/link/size/credential protections apply to PLAN writes.

The bounded planner loop returns tool results and denials to the model,
redacting known credentials from error messages and replayed tool-call history.
It accepts OpenAI JSON-string function arguments and one JSON tool payload
embedded in text (including SGLang/DeepSeek-style fenced output). Arguments
must still be an object with string `path` and `content`; parsing never grants
an additional tool or path. Malformed calls are retried once with a short
tool-only instruction. Another failure produces a visible error and unverified
planning stubs, not an exception that ends `/run` in the shell.
It accepts the existing JSON plan format or a complete written TASK, which
finishes planning in that tool response without another confirmation turn.
Written tasks accept case-insensitive title, `Original Ask`/`Ask`,
`Acceptance Checks`/`acceptance_checks`, and `Allowed Files`/`Files allowed`
headings in any order, with other sections such as Scope retained. They must
contain the issue Ask text and pass the
same title, metadata, acceptance-check, allowed-path, and routed-model checks.
Wrapped Markdown list items are joined before validating checks. Root
RECIPE.yml, TASK.md, ESTIMATE.md, and PLAN.md entries describe planner bookkeeping only;
they are excluded from application scope and never writable by the coder.
The harness finalizes the managed recipe/task/estimate through this writer;
recipe topology and estimation evidence remain harness-owned, not model grants.
Ask comparison uses normalized whitespace and backticks, ignoring issue
template headings/comments and task-metadata boilerplate. A nonempty Ask
section may contain the human issue title or first substantive body line rather
than the entire issue template. Annotated `Ask (unchanged)` headings work too.
Missing/empty Ask and explicit file-scope or routed-model violations still fail.
Coder context, estimation, file allowlists, and reviewer evidence use the same
task section parser. A validated cached recipe/task can be handed directly
to the coder without giving the planner an app-code tool.
Draft-only Ask planning outside an issue worktree has no file tool. Empty
endpoints still use the deterministic stub; trusted planning artifacts are
written without any model calls or app-code changes.

## Coder file scope

Malformed coder tool-call arrays/JSON arguments get exactly one tool-only repair.
If repair is still malformed, a deterministic edit is available only for an
explicit Status task whose application scope is exactly README.md and whose
read/write tools are enabled. It adds `## Status` and one neutral body line
without changing existing prose, preserves LF/CRLF and EOF conventions, and is
idempotent. A pre-existing multiline Status section is not silently rewritten.
Both paths still run required tests and excellence; network errors, denied
tools, broader tasks, or failing verification are not success fallbacks.
Live logs and RESULT.md say `model` or `deterministic-readme`; RESULT also lists
changed files and test exit. The coder never invokes an App publisher itself.

File paths must be relative to the issue worktree. Absolute paths, path
escapes, symlinked components, and paths resolving outside the worktree
fail rather than being followed. Ambiguous Windows components (trailing
dots/spaces) and alternate data streams are refused on every platform.
`write_file` checks existing path components before creating missing
parent directories, so a denied symlinked parent does not create
directories outside the worktree.
`write_file` also requires an exact file
or directory pattern under `## Files allowed` in the generated
[`TASK.md`](../templates/sdlc/TASK.md); a broad pattern never overrides
the denylist.
For a named vLLM profile the example endpoint is
`http://127.0.0.1:8000/v1`; the coder POSTs to `/chat/completions`
with only the selected five function tools. Its bounded context pack
contains the principal, TASK.md, AGENTS.md, requested skill excerpts,
up to the last 20 memory entries, and allowed paths. With an empty
`llm.base_url` it uses the deterministic stub: no model request, tool
calls, tests, or fabricated code diff, and RESULT.md reports that checks
were not run.

All file tools deny `.env`, `.env.*`, and `*.env` anywhere, `*.pem`, vault
storage under `.roster/vault`, Git metadata,
`agent-policy.yml`, `.github/workflows`, or the pinned
`vendor/github-agent-contracts` dependency. Root `ASSIGNMENT.md`,
`RECIPE.yml`, `TASK.md`, `PLAN.md`, `CONTEXT.md`, `RESEARCH.md`, `ESTIMATE.md`,
`RESULT.md`, and `REVIEW.md` are managed files that the coder
cannot rewrite. `list_dir` refuses protected paths and hides their names
when listing a parent. Policy and workflow bodies are no longer readable
as task context; both reads and writes are denied.
The human evaluation ledger `.roster/evals.jsonl` and seat notebooks under
`.roster/memory` are also write-protected,
even when TASK.md grants broad write scope.
Live `.roster/runs/*.log` files are managed and write-protected too, and their
append-only activity is excluded from verification and publication diffs.
Run logging records only the tool name and requested path, never arguments
containing file bodies, search queries, tool results, or test output.

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
success or publishes. The [excellence gate](EXCELLENCE.md) verifies actual
diff paths and secret checks before RESULT.md and again before publication;
a test process cannot bypass those checks by editing outside task scope.
`tools.run_test: false`, selectable in [onboarding](ONBOARDING.md), removes
the model tool and denies automatic execution. A test-required task fails
before a model request; only an explicit TASK.md `tests: none` waiver can
run without tests. The `tools.internet` preference is stored only and adds
no internet or search tool.
After a final summary, the configured coder runs final tests and checks
excellence before ending its loop. Failed final tests return redacted,
bounded diagnostics for another tool turn while the configured turn
budget remains. An unsafe path or detected secret stops immediately;
a summary alone is never a passing gate. The harness checks the verified
worktree snapshot again after recording memory. Successful model and
reported usage feed the run journal and contracts-compatible AI-Run;
vLLM's journal provider is `vllm` but its AI-Run provider is `local`,
while an explicitly identified GitHub Copilot endpoint uses
`github-copilot` for both.
Tests themselves execute project code; these
application-level guards are **not** an OS sandbox. See the
[threat model](THREAT_MODEL.md) for deployment boundaries.
The reviewer reads task-allowed diff evidence after RESULT.md, receives no
`write_file` (or any other model tool), and only the harness writes REVIEW.md.
See the [reviewer gate](REVIEW.md); reviewer comments do not grant merge or
publish authority.
