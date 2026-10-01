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
| `TEXT` (no slash) | Classify a direct local ask without `gh` or issue creation. In a local Git worktree, slices print the task summary and run planner/coder/reviewer to RESULT.md and REVIEW.md; features and initiatives write PLAN.md and stop. |
| `/ask TEXT` | Classify Ask, then create its parent issue through `gh`, or save an offline slice TASK/recipe or planning-only PLAN. A `clarify` Ask stops with outcome/file-scope guidance. |
| `/model [MODEL]` | Show the current model or persist a new one to ignored `.roster/config.yml`. |
| `/effort [l|m|h|x|none]` | Show effort or persist an explicit override to ignored `.roster/config.yml`; it wins over mode/retry defaults. |
| `/run N [--auto-model] [--confirm]` or `/run --issue N [--auto-model] [--confirm]` | Classify before seats. Slices print outcome, allowed files, checks, and effort, then automatically continue through coder/read-only-reviewer in the same run. Only `--confirm` pauses after the summary; no `--auto` or second `/run` is required. Feature/initiative writes PLAN only; clarify stops. Auto-model chooses a registered fleet profile without rewriting the saved default; no eligible profile uses the deterministic stub. |
| `/status [N] [--offline]` | Show an issue, its open branch PR, and worktree path. Defaults to the last run or created issue; offline reads only the cached assignment and reports PR state as unknown. |
| `/log N` | Tail up to 50 safe metadata lines from each local `.roster/runs/roster-N-*.log`, without network or seat execution. |
| `/debug on` or `/debug off` | Enable or stop testing metadata logging for this process only; never write config or change environment variables. |
| `/log debug` | Tail up to 50 validated JSONL events from this process's most recent debug file, even after debug is off. |
| `/eval TARGET accept\|reject\|rework 1-5 y\|n [--minutes N] [--comment "TEXT"]` | Record the [human retrospective](RETRO.md), including actual minutes and local feedback. |
| `/publish [SUBJECT] [--model MODEL] [--skip-review]` | Publish with an unchanged passing REVIEW.md, or explicitly bypass that verdict. After `/run N`, the default subject is `feat: issue N`; a confirmed merge comments on the still-open issue with PR URL and model ID. Local asks default to `feat: local ask`, publish from their worktree, and never comment on an issue. Otherwise supply a conventional subject and declare the GHCP model with `--model` or `AI_MODEL`. Completed seat metadata wins over the flag. |
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
For a slice, when the existing RECIPE/TASK validate for the issue, `/run N` preserves them
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

`Ask kind: clarify | slice | feature | initiative` is determined before seats,
separately from task class and model/difficulty. A README one-liner is a slice;
"build an orchestrator" is an initiative, not a README edit. Features write
2-5 child issue drafts in PLAN.md; initiatives write outcomes, waves, and
issues in PLAN.md only. Waves are issue labels (`wave:N`), not another queue.
No coder/reviewer/tests/publisher or automatic child issue creation occurs
on these planning-only runs. Review the drafts on GitHub and create bounded
slice issues, then `/run` those issues; rerunning the parent only plans again.

The slice senior-team default uses Ask, TASK outcome/scope/checks, two small
skills, final tests/excellence, read-only review, and a human eval hint.
Classified slices do not load research/implementation packs, even at
feat difficulty4+. Whether file scope is declared or inferred from named files,
the validated TASK summary streams before coder starts and execution continues
automatically. Use `/run N --confirm` to stop after the summary, then run
without that flag when ready. CLI `roster run --issue N --confirm` has the same
pause; it cannot be combined with `--publish`.
Multiple Outcomes now take the feature PLAN path instead.
A planning-only handoff or clarification cannot publish, even with a
review bypass. No file scope is invented for an Ask that names no files.

`/run N` streams one plain-language stderr line per action event, independently
of its final summary. Planner start/model request says
`Writing the plan: outcome, allowed files, and checks.` Coder reads say
`Reading README.md before editing.`, writes say `Saving README.md.`, model
requests say `Drafting the change.`, and tests say `Running tests.`
Configured coder requests include the chosen effort, for example
`Drafting at low effort. Model prior: strong.`
Reviewer start says `Checking the diff against the task.` These are status
projections only: they do not add tools, checks, edits, or work. A stub coder
says `Preparing the task summary.` rather than claiming an implementation.

Unsupported LLM finish reasons are named in the shell and run log without
printing response bodies. `stop` and `tool_calls` are accepted. The first
`length` logs `Response truncated. Retrying.` and uses one extra model turn
with half the completion cap and a concise-response instruction. Repeated
truncation or another unsupported reason fails review with that reason,
preserving any completed README write. This does not consume a test repair.

Failed tests keep the coder running for up to four repair attempts after the
initial failure. It reads a bounded, redacted summary, repairs allowed files
plus any specifically identified failing test, and reruns `node --test`.
Each repair streams `Tests failed. Repair 1 of 4.` with its actual attempt.
Excellence and reviewer wait for green tests or exhaustion of that budget;
exit 1 is never done. Timeouts and denied paths remain failures, and initiative
plans still stop after PLAN.md.

The existing timestamped technical metadata is appended **only** to the issue repository's
`.roster/runs/roster-N-coder.log`: seat starts, model/endpoint host, HTTP
phase/status/error class, tool names/paths, managed file writes, mode, and elapsed
milliseconds. No prompts, completions, file bodies, keys, or upstream error
messages are logged. `/status N --offline` reads a bounded local tail and shows
the last seat and last complete line; it does not contact GitHub.
Offline status also shows the expected issue branch, last error class, whether
TASK/RECIPE/PLAN/RESULT/REVIEW exist, and the latest matching JSONL model and prompt/
completion counts. Unknown counts stay `-`, never invented zero. `/log N`
reads every matching seat log with a bounded tail. Model/mode/status/elapsed and
managed-write metadata stays log-only; stderr is not a technical transcript.
Explicit `/log` and `/status` diagnostics retain the original technical evidence.

### Opt-in testing log

Debug logging is **off by default**. Start the shell with `roster --debug`, use
the leading flag for a command (`roster --debug run --issue N`), set
`ROSTER_DEBUG=1`, or enter `/debug on`. `/debug off` stops new events even when
the environment originally enabled logging. These choices are process-local,
not a saved config preference.

The first enabled seat event creates
`.roster/logs/debug-<opaque-process-session>.jsonl` in the issue repository.
Disabled logging creates no debug file or logs directory. That directory is
gitignored; consumer repositories must ignore `.roster/logs/` too, otherwise
logging fails explicitly rather than creating a publishable debug file.
It is excluded from task snapshots and publication, and denied to coder tools.

Each line has `time`, `issue` (or null for local asks), `seat`, `phase`,
`tool_name`, `path_class`, `finish_reason`, `test_name`, `exit_code`, `repair`
(`{"n":1,"of":4}` for test repairs), and `elapsed_ms`. Inapplicable fields are
null; elapsed time starts when the process logger is created.
A refused vendor list is `tool-denied` with `path_class: "vendor"`,
not a stored path or error body. `test_name` is the controlled runner label
`node --test`, not copied test-output titles. Finish metadata uses known
reason labels, `redacted`, or `unsupported`; arbitrary provider text is not
copied. Human/technical finish diagnostics retain their existing named reasons.

The file never receives prompts, completions, file or test-output bodies,
token counts, credentials, PEM paths, environment values, model IDs, or
endpoint addresses. Enabling debug adds **no JSON lines to shell output**:
the existing one-line human status remains unchanged. Only an explicit
`/log debug` request displays a validated tail. Tail reading refuses malformed
or extra fields rather than printing untrusted content.

### Local LLM cold starts

DGX Spark / SGLang may need 10-15 minutes for the first `chat.completions`
after more than two hours idle. Local loopback/private-IP endpoints default
to a **20-minute** total HTTP deadline; cloud/public endpoints use **120s**.
Both include response JSON parsing and the bounded 429 retry. The optional
`llm.request_timeout_ms` positive integer overrides either default:

```yaml
llm:
  # Keep the other existing llm fields.
  request_timeout_ms: 1200000
```

Long default requests use Node HTTP/HTTPS directly, without npm dependencies,
so Node 20 fetch's five-minute header/body deadline cannot cut short this
configured wait. Injected transports remain supported and must honor the
provided abort signal/deadline themselves.

While an HTTP request is in flight, the technical log still records waiting
every 30s, for example:

```text
2026-10-01T12:00:00.000Z seat planner waiting host=192.168.1.48:8888 elapsed=90s cold-start up to 15m
```

After more than 30s, each waiting event prints only
`Still waiting on the model. Local hardware can take minutes after idle.`
to stderr. Timeout prints
`The model did not answer in time. It may still be waking.`
The detailed host/elapsed/retry metadata remains in the technical log and
the actionable error report.

Only seat, host/port, and elapsed time are included in waiting log records: no URL paths, credentials,
prompts, completions, or file bodies. The waiting timer stops on completion,
failure, or timeout. A local timeout says the host may still be warming and
prints the retry command, for example `roster run --issue 92` (shell `/run 92`).
Auto-model retries retain `--auto-model`. Timeout is an endpoint failure,
not a bad TASK: valid TASK/recipe files are preserved and the next run reuses
them without another planner call. No failed or unverified run may publish.

`roster doctor` remains offline. For an opt-in readiness/warming probe, use
`roster doctor --warm`; it GETs the configured `/models` (normally
`/v1/models`) with the same timeout policy and logs `warming` host/status
metadata. It does not write config, Task artifacts, or code. A successful
models probe does not prove model weights are warm; the first chat may still
need the full cold-start wait.

`/model MODEL` and `/effort h` validate and atomically replace those
fields in the private config, keeping other fields and comments. `/effort`
also writes `llm.effort_override` so a human choice is not lost to automatic
selection. `/effort x` sends max to local DeepSeek-V4.1 and xhigh to cloud
except for docs slices, which cap all choices at high;
`/effort none` disables thinking. Remove the override field in private config
to return to difficulty/model-prior defaults. The
updated values apply to the next `/run` in this shell. With no selected
endpoint or profile, setting a model alone still leaves the stub active.

Every ask uses difficulty versus the [model capability prior](CAPABILITIES.md):
strong models use low at difficulty1-2, while difficulty4-5 uses high.
This is independent of task class or filenames. Low-difficulty docs slices
retain `max_tokens: 2048`; feature/initiative plans retain 4096.
Every slice denies reads outside TASK.md and its allowed files, including
tests/fixtures and harness sources unless allowed explicitly.
A new `/run` after a failing REVIEW
uses the preceding journaled coder effort raised one supported tier (never
past max/xhigh, or high for docs slices), unless an explicit override exists. The task and minimum
context are unchanged. No retry/model call is scheduled just by selecting
effort, and no `reasoning_content` is persisted in seat memory.
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

After a passing reviewer, the run prints its worktree and `git diff --stat`,
then the exact human evaluation command for `roster-N-coder`. Issue publication
uses the coder model and token counts from its persisted runs JSONL row, not
Copilot defaults; a missing/mismatched row refuses publication. Bodies use
`Refs #N` and the issue remains open. After a confirmed merge, the human runs:

```sh
roster eval roster-N-coder accept 1 n --minutes M
```

Replace N with the issue number and M with actual minutes. This is only a hint:
Roster does not execute it or generate AI-Eval on the human's behalf.

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
