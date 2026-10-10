# Builtin seat tools

The configured builtin coder can use only the function tools listed in
`seat.tools` in the [Roster config](../roster.config.example.yml). The builtin
planner has only the artifact-scoped writer described below; reviewer has no
model-invokable tools. The offline stub makes no model tool calls or test runs.
The opt-in [testing log](REPL.md#opt-in-testing-log) records tool start/result
metadata, including refused path classes and test exit codes, without arguments
or output bodies. Result events add no human status lines. Debug log files are
managed and protected from coder reads/writes, even with broad TASK scope.
The [tool implementation](../src/runtime/tools.mjs) offers the coder exactly
five functions:

| Tool | Input | Result |
| --- | --- | --- |
| `read_file` | `{ "path": "README.md", "max_lines": 200 }` (`max_lines` optional) | UTF-8 text from a regular worktree file, optionally limited to the first positive integer number of lines. |
| `write_file` | `{ "path": "src/app.mjs", "content": "..." }` | Creates or replaces task-allowed UTF-8 text; returns path and byte count. |
| `delete_file` | `{ "path": "probe.mjs" }` | Deletes a regular file inside TASK scope, or an untracked unprotected scratch file; returns `{ path, deleted: true }`. |
| `list_dir` | `{ "path": "src" }` or `{}` for root | Sorted entry names and types, excluding protected entries. |
| `run_test` | `{}` | Runs `node --test` from the worktree root; returns `exit_code`, captured `stdout`, and `stderr`. |
| `search_text` | `{ "query": "literal text", "path": "src" }` (`path` optional) | At most 50 `{ path, line, text }` matches and a `truncated` flag. |

Every slice limits file reads to TASK.md and its allowed-file patterns, regardless
of task class, difficulty, or filename. Directory listings expose only allowed
files and their ancestor directories; recursive searches visit only that scope.
Explicit reads, listings, or searches outside it fail with a TASK scope denial.
Tests/fixtures and harness sources are denied unless TASK explicitly allows
their paths, with one narrow repair exception: a regular test file identified
by failed Node test diagnostics may be read and repaired. This does not grant
the tests directory, sibling tests, imported app code, or harness sources.
Secrets, Git metadata, human-owned policy/workflows, and contracts
remain protected even with broad TASK scope. Test execution remains a separate
permission; read scope does not waive the acceptance checks.

Denials come in two classes. Security-boundary denials stop the coder at once:
path escapes, writes to managed or protected files, strict-scope writes outside
TASK scope, secrets, Git metadata, policy, workflows, contracts, symlinks, and
unsafe web targets. Scope and usage mistakes (reads outside the slice, unmatched
edits, disabled tools, disallowed commands, exceeding the scope-expansion cap)
return `Denied: ... Continue` so the model can correct course without seeing the
denied content. The same usage denial a third time stops the run with
`(repeated after 2 denials)`.
A call to a tool name that is not a Roster tool at all gets one correction
listing the offered tools; a withheld Roster tool, or a second invented
name, still stops the seat and names the tool. A call id a gateway reuses
from an earlier turn is replaced with a fresh id instead of failing.

Tiered write scope: with `seat.scope_expansion` above 0 (default 3), a coder
`write_file` or `edit_file` on an unprotected repository file outside TASK.md
succeeds and returns `scope_expanded: true` with a note. The file is recorded
once (a failed `edit_file` match does not use a slot) and survives a route
recovery. It is accepted by the excellence gate, sent to the reviewer under
"Files outside planned scope", staged for publication, and listed in the PR
body. README-only and single-file bounded tasks stay strict.

At the cap, the denied path emits a `scope-limit` event and the coder result
lists it in `scopeBlocked`. If that coder attempt fails, the orchestrator
re-scopes instead of stopping. It raises the budget to
`max(2 × budget, budget + blocked + 1)`, capped at 16, at most twice, logs
`Re-scope N of 2`, and resumes in a fresh context with the same model and the
worktree edits. Strict scope (`0`) and hard-deny paths never re-scope.

Scratch cleanup: the coder may remove its own probes with `delete_file`. It is
offered whenever the coder may write (a recipe that grants `write_file` or
`edit_file` grants it too) and refuses protected, harness, `.roster`, and
Git-tracked files outside TASK scope. When the final excellence check finds
only unprotected files outside TASK scope in the diff (for example a leftover
`probe.mjs`), the coder gets up to two in-loop corrections to delete them or
rewrite them as recorded scope expansions instead of failing the run.
Protected diff paths still stop the run.

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
A fenced or raw JSON plan in the same assistant message is enough when the
written TASK.md is missing a heading; the harness then finalizes TASK.md.
An incomplete draft with turns remaining gets an explicit heading repair
instead of a silent success from write_file.
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

For difficulty1 `docs` with exactly README.md as application scope, the
harness offers only `read_file`, `write_file`, and permitted `run_test`.
The read schema/runtime guard accepts only root TASK.md and README.md;
the write schema accepts only README.md. Reads may precede the edit, but a
successful README write is required before tests or final completion,
including tasks that explicitly waive tests. Directory listing and search
are denied on this class, so requests for RESEARCH.md, tests/fixtures, or a
repo-wide `search_text` cannot enlarge the context. Identified failing tests
are added to the read/write schemas only after a failed test run.
After a successful write when TASK permits exactly one application file, the
harness runs the task checks immediately and only once. Green checks switch to
summary-only mode, so later read/search requests are denied without executing
or invalidating the passing result. Failed checks reopen scoped reads and
searches for the bounded repair loop.
A tool denial terminates coding with an unverified result, not file content
or evidence of completed work, except for requests refused after those
post-write checks are already green. Other task
classes/difficulties/scopes retain the existing behavior below.

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

The coder's tools enforce read-before-write and search-before-create
([#295](https://github.com/R4yHarris/github-agent-roster/issues/295)):

- `write_file` cannot replace an existing code file (`.js`, `.mjs`, `.cjs`,
  `.ts`, `.jsx`, `.tsx` and related) unless this session read or wrote it
  first. `edit_file` needs no prior read, because its exact `old_string` match
  already proves the content. Docs such as README.md are exempt.
- A new `src/**/*.{js,mjs,cjs}` file is refused until the coder has run
  `search_text`, `glob_files` or `list_dir`. Slice-only and one-file tasks are
  exempt, because those tools are withheld from them.
- A new `src` file is also refused until the coder has read every module listed
  under "Earlier waves delivered". Those modules stay readable even when reads
  are limited to the slice.

Each refusal is a correctable tool error, so the turn continues.
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
`vendor/github-agent-contracts` dependency. The human-owned
`.roster/hooks.yml` manifest and `.roster/hooks/` scripts are also denied;
see [lifecycle hooks](HOOKS.md). Root `ASSIGNMENT.md`,
`RECIPE.yml`, `TASK.md`, `PLAN.md`, `CONTEXT.md`, `RESEARCH.md`, `ESTIMATE.md`,
`RESULT.md`, and `REVIEW.md` are managed files that the coder
cannot rewrite. `list_dir` refuses protected paths and hides their names
when listing a parent. Before any tool runs, a path containing `..`, an
absolute path, or anything under `vendor/` is refused; the shell prints
`Refused: outside the worktree.` and the run log records only the tool name.
A diff that touches a `vendor/` path, including the contracts submodule
gitlink, fails the excellence gate and review even when tests pass. Policy and workflow bodies are no longer readable
as task context; both reads and writes are denied.
The human evaluation ledger `.roster/evals.jsonl` and seat notebooks under
`.roster/memory` are also write-protected,
even when TASK.md grants broad write scope.
Live `.roster/runs/*.log` files are managed and write-protected too, and their
append-only activity is excluded from verification and publication diffs.
Run logging records only the tool name and requested path, never arguments
containing file bodies, search queries, tool results, or test output.
The shell projection is a single transcript line per tool event, such as
`read_file README.md`, `write_file README.md` or `run_test`, collapsed with a
count when the same tool repeats on the same target.
Paths remain redacted and single-line. Timestamped tool/HTTP/model metadata
stays in the run log, not the live stderr status. No additional work is
scheduled by status reporting.

`search_text` is a case-sensitive, fixed-string grep, not a regex or a shell
command. It walks regular worktree files in sorted directory order, uses
the same read/list guards, skips binary files containing NUL and directory
symlinks, and never traverses protected entries. `path` can select a file or
subdirectory; an explicitly requested unsafe path fails rather than silently
returning no matches. Line numbers are one-based. An additional match beyond
the 50 returned lines sets `truncated: true`; exactly 50 matches is not
reported as truncated. No subprocess is used for search and no npm dependency
is required. Empty, NUL-containing, or multiline literal queries are denied
before a search tool-start event.

`run_test` accepts no arbitrary command or shell arguments. It strips the
configured model API key and App/GitHub credentials from the child
environment, marks the child as `ROSTER_SEAT=coder` to preserve the human-only
evaluation boundary, and captures test output. Docs-only scope, including
`README.md`, skips tests and checks the file; it never spawns a fallback suite.
A code slice's own `run_test` calls run the tests that cover
its files plus any failing tests already surfaced for repair, using the existing
code verification policy: a 15-minute process cap and a 15-minute Node test-file cap.
Node 20 also applies `--test-timeout` to aggregate test-file lifetime, so a healthy
slow lifecycle fixture must not inherit a docs-only or 2-minute aggregate cap.
This does not guarantee a separate 2-minute per-subtest deadline. This Eve rollout
verification fix (#473) follows feature spec sections [5.4, 5.5, and 5.8](FEATURE_SPEC.md#54-execution):
scoped execution, unchanged verification gates, and bounded operation.
A module test split into `tests/<module>.<topic>.test.mjs` shards counts
as `tests/<module>.test.mjs`: the run includes every shard, and the coder may
update any shard (see [testing](TESTING.md)). Final verification, which only the harness can request, runs the full
`node --test` suite with the same 15-minute process and test-file caps,
because a slice can break tests in files it never planned to touch.
Full-suite failures outside Allowed Files are rerun alone (a pass marks a
flake), then once at the base commit in a temporary detached worktree. Initialized
submodule content is copied into that worktree, so missing dependencies cannot make a
regression look pre-existing. A test
that passes at base is a regression this change caused: it becomes a repair
file the coder must fix, by updating the test to the intended new behavior or
by fixing the implementation. Only failures that also fail at base are
reported as pre-existing and left alone. A flake is transient, not pre-existing:
when every failure is an outside file that passes alone (a timeout or
cancellation under full-suite load), the run counts as green with
`transient_files`, the original `full_suite_exit_code`, and a stderr note, so
the coder is not steered around a failure that does not exist. If the base
check cannot run, outside failures that did not pass alone are treated as
pre-existing, as before.
`run_test` returns `failing_files` (what failed in this run) separately from
the cumulative `repair_files` write grant. Classification uses the current
failures, so a test the coder already fixed cannot block the pre-existing-only
steering path. Failed-test evidence shown to the coder and in RESULT.md keeps
the reporter's `failing tests:` section (or `✖`/`not ok` blocks) and counts,
plus the stderr tail, rather than the head of a long list of passing lines.
A nonzero Node exit is a failed tool result rather than completion,
so the coder receives its summary and a fresh repair attempt. Both scoped and
full code verification use the same 15-minute `--test-timeout`; Node timeout
failures remain failed checks, and the process cap bounds the whole invocation.
A whole-process timeout is still an explicit
error. Final verification must pass before a configured run reports
success or publishes. The [excellence gate](EXCELLENCE.md) verifies actual
diff paths and secret checks before RESULT.md and again before publication;
a test process cannot bypass those checks by editing outside task scope.
One infrastructure exception is missing contracts scripts: a declared
submodule is checked before tests, and a dependency-only missing-module
diagnostic produces `Contracts submodule was not initialized` rather than
a slice repair or vendor listing. RESULT records the blocked prerequisite;
verification and publication remain unavailable. Real or mixed failures
are not exempted.
`tools.run_test: false`, selectable in [onboarding](ONBOARDING.md), removes
the model tool and denies automatic execution. A test-required task fails
before a model request; only an explicit TASK.md `tests: none` waiver can
run without tests. The `tools.internet` preference is stored only and adds
no internet or search tool.
After a final summary, the configured coder runs final tests. Every failed
test run (model-requested or final) starts a repair, up to four after the initial
failure, independent of already consumed tool turns. Remaining calls in a failed
test batch are deferred so the model reads the summary before further edits.
If a repair consumes its tool-turn allowance without a summary, the harness
reruns tests: another failure starts the next repair, while green tests request
a final summary without additional tools. The first failed check cannot end
the run just because the ordinary tool-turn allowance was already consumed.
Each repair logs `Tests failed. Repair 1 of 4.` with its actual attempt number.
After the fourth unsuccessful repair, RESULT records budget exhaustion and
review fails without model inference. Excellence runs only after green tests
or terminal failure; the reviewer runs after that finalized result.
An unsafe path or detected secret cannot pass final verification;
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
