# Interactive Roster shell

Run `roster` with no arguments from a Git worktree and an interactive terminal
after [installing it](INSTALL.md). The screen has three regions.

The **banner** prints once at startup: the product and its version, then
`node MAJOR · SHELL · WORKTREE · BRANCH`, then `endpoint STATE · MODEL · HOST`,
then `contracts VERSION · update STATE · warnings COUNT`. The version comes from
`package.json` unless it is the `0.0.0` placeholder, in which case the roster
checkout is described from Git instead. A configured `base_url` prints
`endpoint configured`, a successful probe prints `endpoint ok` and a failed
reachability check prints `endpoint down`, never a dash. Each fact is checked
separately, an unknown fact prints `-` rather than a guess, and every failed
check adds its own bright-yellow line without stopping startup. The release and
endpoint probes are opt-in (`ROSTER_BANNER_CHECKS=on`) and time out after one
second, so an unchecked release prints `update skipped` and the whole banner
finishes in under two seconds.

The **transcript** scrolls above the rail with one short line per step: a phase
line, a tool line, a waiting line and a verdict line. Repeated identical tool
calls collapse into one counted line such as `read README.md · 2`, and the
waiting line is rewritten in place rather than repeated. Model prompts,
completions and file bodies are never printed.

The **rail** is pinned between two bright-cyan rules directly above the
bright-cyan `roster> ` prompt, for example
`#108 draft │ deepseek-v4.1-flash l │ - / 1.0m │ 7m26s`. It carries the issue
and phase, the configured model id with its effort letter, the context counts
and elapsed time; an idle session shows `idle` and `-`. Unknown usage prints
`- / 1.0m` with no bar; a real count prints `5.2k / 1.0m` behind a ten-cell bar
that is green under 50%, yellow to 80%, orange to 95% and red at or above 95%.
Token counts are never estimated from character length. A failed seat turns the
first field red and replaces elapsed with the finish reason, and `/debug on`
adds a trailing `*`. The endpoint, the branch and the word `debug` stay off the
rail. Narrow terminals drop the context field, then the model, keeping the
issue and elapsed fields. Values are
white, waiting and warnings are bright yellow, failures bright red and passes
bright green; no other colours, dim text or emoji are used. The rail repaints on
seat, tool and test events without a keypress, and redraws preserve Ctrl+C,
history keys and Tab. A redraw pauses readline, repaints only the two rules,
the rail and the debug detail, flushes stdout and calls `prompt(true)` before
returning. Every transcript append follows the same pause, write, flush and
prompt sequence. The input row remains owned by readline, so a one-second tick
needs no key event and never joins the rule to a half-typed command or consumes
typed characters.
`/statusbar off` removes both rules and the rail for this
process and leaves the plain prompt.

Seat events reach the screen through one synchronous sink shared by the runtime
loop, the tool runner and the shell. Only five kinds of event may touch the
screen: a phase (`plan`, `draft`, `test`, `review`) with the issue number, a
tool with its name and target, a wait rewritten in place with elapsed seconds,
a verdict with pass or fail, the finish reason and the files written or
`no write`, and usage. Usage updates the rail bar and `/usage` only and never
prints a transcript line; unknown usage leaves the bar empty and shows `-`
rather than an estimate from character length. Repeated tool events coalesce
within 200 ms and the rail ticks at most once a second, so painting never waits
on the network. `/debug on` adds one `thinking … │ max_tokens …` line under the
rail, never inside the prompt body.
`--help` still prints the existing CLI usage and exits 0; empty arguments
with non-TTY stdin print that usage and exit 2. Flags remain available for
agents and CI.

| Command | Behavior |
| --- | --- |
| `TEXT` (no slash) | Classify a direct local ask without `gh` or issue creation. In a local Git worktree, slices print the task summary and run planner/coder/reviewer to RESULT.md and REVIEW.md; features and initiatives write PLAN.md and stop. |
| `/ask TEXT` | Use the same local planner/coder/reviewer path as plain text; never create a GitHub issue. A bounded slice reaches RESULT and REVIEW, while clarify and initiative/feature planning retain their existing boundaries. CLI `roster ask` keeps its separate issue/draft behavior. |
| `/plan TEXT` or `/run N --plan` | Enter opt-in plan mode: explore with read-only source tools and write only PLAN.md. The rail remains in the plan phase. Enter accepts a bounded slice, validates its original Ask/file scope, creates the normal TASK/recipe handoff and starts coder in the same worktree. `/stop` keeps the plan and starts no coder. Feature/initiative plans remain planning-only. |
| `/model [ID\|clear] [--save]` | Show the session model and host, or change only this process. `--save` explicitly writes the private model setting; `clear` lets the next run route. |
| `/effort [l|m|h|x|none|status]` | Show or select session effort without writing config; docs still cap at high and their truncation retry drops reasoning. |
| `/provider` | Show the selected profile name and host, never a key. |
| `/fleet` or `/fleet list` | List configured profile IDs, hosts, models and context capacities; no endpoint paths or catalog notes. |
| `/fleet use ID` | Select an endpoint/model/context for this session only; do not write the catalog or saved default. |
| `/fleet probe [ID] [--set-model [MODEL]]` | Read-only GET of `/v1/models`, listing IDs and reported context. Only explicit `--set-model` saves a listed model to private config; the shell does not rewrite fleet entries. Ambiguous model lists require a supplied ID. |
| `/fleet add FLAGS` | Use existing non-TTY add flags. Terminal model/context questions use this readline interface and are not recorded as commands or submitted as asks. CLI interactive add behavior is unchanged. |
| `/run N [--auto-model] [--confirm]` or `/run --issue N [--auto-model] [--confirm]` | Run an existing issue, initializing contracts before tests. Slices print the summary and continue. `--confirm` pauses; Enter resumes the prepared task in its worktree without another issue lookup, while `/stop` cancels. Feature/initiative writes PLAN only; clarify stops. Auto-model does not rewrite the saved default. |
| `/retry` | Rerun the last plain or slash Ask or issue run in the same registered worktree. Preserve app changes, validate the unchanged assignment and branch, archive managed run artifacts, reuse a valid TASK/recipe, and never call worktree add for a retry. |
| `/stop` | Immediately cancel an in-flight seat or a confirmed handoff, like one Ctrl+C. |
| `/steer TEXT` | Interrupt only a drafting coder model call, discard its stale response/tool calls, and send queued lines plus this text as the next human instruction. It cannot modify TASK or widen Allowed Files. Planner/reviewer/test phases cannot be steered. The shell says `Steering the coder.`; Ctrl+C still cancels the run without an instruction. |
| `/status [N] [--offline]` | With no number, show cached issue, branch, seat, state, model, host, effort, last finish reason, last test name, and review. Current and previously run issue snapshots use no model/GitHub request. An uncached explicit issue uses the existing status reader; `--offline` prohibits GitHub. |
| `/history` | Show the last 20 stored safe commands, without vault or secret lines. |
| `/resume` or `/resume N` | List registered local issue runs (number, title, last seat/state, branch), or reconstruct the assignment locally and continue in that exact worktree. A valid TASK/recipe skips planner; no new worktree or GitHub issue lookup is required. It attaches local artifacts, not a foreign running process. |
| `/worktrees` | List registered issue worktree path, branch, last known seat and Git dirty/clean state. Duplicate issue branch registration is refused; missing seat evidence stays `-`. Creation initializes contracts recursively before tests. |
| `/batch` | Refuse with `One seat at a time. Worktrees are isolated.`; no parallel seat or second board is started. |
| `/recap` | Print one human metadata line with TASK outcome/files, last test exit, review and finish reason. Never print RESULT completion text. |
| `/btw QUESTION` | Ask the configured model one read-only question about current task metadata. It offers no tools, refuses tool requests, and prints only the answer; no task, memory, measured-seat state or publication body is changed. It can answer alongside a running seat without steering it. Truncation fails rather than making a second request. |
| `/context` | Show this process's last response-backed seat: provider, actual model, actual request effort, input/output counts, declared context capacity, finish reason, character pack budget and whether prior feedback was included. Missing counts are `-`; no body or environment usage is substituted. Side questions do not replace the measured seat. |
| `/usage` | Print one read-only panel for the current session: full model, endpoint, prompt and completion tokens, context max, effort, finish reason, tool calls, elapsed time, whether thinking was disabled and the outbound completion cap. Unknown values are `-`. |
| `/map` | Write ignored `.roster/map.md` with TASK-named paths and the top two directory levels, capped at 80 filename-only lines (no file bodies). Protected/private paths and symlinks are omitted. Non-docs difficulty4+ coders may load/read it; low difficulty and all docs tasks cannot. It grants no extra product path permissions and coder writes are always refused. |
| `/checkpoints` | List the current task's pre-write checkpoint number, coder seat, short status and time. |
| `/rewind N` or `/undo` | Restore checkpoint-covered product files and remove newly created files in that same task scope. Keep PLAN, TASK, recipe, result and logs. Undo selects the latest checkpoint only. Stop the seat first; any open/closed/merged PR on the branch refuses rewind, and unavailable PR verification fails closed. Verification/review are invalidated after restoration. |
| `/statusbar on\|off` | Toggle the pinned rail and its two rules; the default is on and the setting is process-local. The plain-text prompt remains available while the rail is off. |
| `/log N` | Tail up to 50 safe metadata lines from each local `.roster/runs/roster-N-*.log`, without network or seat execution. |
| `/debug on` or `/debug off` or `/debug status` | Enable, stop or show testing metadata logging for this process only; never write config or change environment variables. |
| `/log debug` | Tail up to 50 validated JSONL events from this process's most recent debug file. Fail closed while debug is off; reenable explicitly before reading. |
| `/issues` | Print up to 100 current-repository open issue numbers and titles only, with a notice at the retrieval limit. Never print bodies. |
| `/waves` or `/waves open` | Validate PLAN child drafts and display wave/title with GitHub-derived todo/running/review/done/blocked states. Plain waves is read-only. Only explicit human `open` creates missing drafts and their planned wave labels; GitHub body markers link them without another board file or local queue. Repeated open does not duplicate linked drafts. |
| `/issue N` | Print cached title, state, branch and PR URL. Reuse cached metadata from runs/listings/status; use GitHub only when that issue is missing from cache. Unknown cached PR state stays unknown. |
| `/diff` | Run filename-only Git diff in the current issue worktree; print tracked changed names, never file bodies. |
| `/eval TARGET accept\|reject\|rework --minutes N --difficulty 1-5 "TEXT"` | Invoke the existing human-only evaluation writer with actual minutes, difficulty and feedback. The compact spelling records `again: n`; legacy positional difficulty/again and `--comment` remain supported. Agent seats cannot invoke the writer. |
| `/publish [SUBJECT] [--model MODEL] [--skip-review]` | Publish with an unchanged passing REVIEW.md, or explicitly bypass that verdict. After `/run N`, the default subject is `feat: issue N`; a confirmed merge comments on the still-open issue with PR URL and model ID. Local asks default to `feat: local ask`, publish from their worktree, and never comment on an issue. Otherwise supply a conventional subject and declare the GHCP model with `--model` or `AI_MODEL`. Completed seat metadata wins over the flag. |
| `/review [--again]` | Run only the existing tool-free reviewer on the current coder result and diff. Product files cannot be edited. `--again` archives a previous untracked managed REVIEW and writes a fresh one; tracked reports are refused. Current verification is rechecked, and changed/unverified products still fail rather than inventing green tests. Failed review blocks publication until the existing explicit review bypass. |
| `/stats [REF]` | Summarize contracts and local AI-Run records, optionally at a Git ref. |
| `/recommend [feat\|fix\|docs\|test] [--difficulty 1-5]` | Read a route without changing any model/default. With no args use the last task class/difficulty, otherwise default to feat/difficulty2. Print its evals/prior reason or insufficient data. |
| `/doctor` | Run the existing six offline prerequisite checks without printing secret values. |
| `/doctor warm` | Run the existing read-only models warm probe using the session endpoint; print only host and status. Ctrl+C cancels it. |
| `/config` or `/config path` | Show validated private config with secret material and PEM paths redacted, or only its resolved path. An absent private file is explicitly identified before showing installed defaults. |
| `/config set KEY VALUE` | Save only effort or positive context budget (character budget, not model token capacity). Supported context aliases include `context.budget` and `seat.context_chars`. Debug/statusbar changes are process-only. Endpoint keys, model selection, PEM paths, secrets and policy are refused; use dedicated model/fleet commands for their deliberate actions. |
| `/vault` or `/vault list` | List vault entry names, never values. |
| `/vault get NAME` | Check whether an entry exists without revealing its value; use piped `roster vault get NAME` to retrieve it. |
| `/vault set NAME` | Read the next line with terminal echo and readline history disabled, then store it in the existing file vault. |
| `/help` or `/` | Show the six command groups: Session, Ask, Model, Board, Human, Settings. |
| `/help GROUP` | List one group's commands, for example `/help Session` or `/help Board`. |
| `/help COMMAND` | Show usage, aliases, flags and one example, for example `/help run`. Lowercase command names win; `/help Model` selects the Model group. |
| `/quit` or `/q` or `exit` | Exit with status 0. Ctrl+C cancels an active run; a second interrupt or idle Ctrl+C exits with status 130. |

## Keys and history

Plain text entered while a task seat is active is held locally (up to eight
lines/4096 characters) and is not automatically sent to the model or started as
another Ask. Explicit steering sends the held text within the same bounded
coder/tool scope. Aborted model calls remain counted as actual attempts and
unknown usage stays unknown; cancelled calls do not consume a completed-turn
allowance. Unsent input is discarded when the run ends, with a notice on normal
completion. Steering metadata logs only the phase, never instruction text.

Wave-labelled issue runs are gated before worktree creation/coding: any earlier
open wave blocks a later wave. Explicitly opened plans use their GitHub marker
to isolate the gate to that plan; manual wave-labelled issues conservatively
use repository-wide earlier labels. Failed readiness lookup blocks execution.
Assignments retain wave metadata so local resume cannot bypass this gate.

Before each configured coder write in a Git worktree, the harness snapshots
task-allowed products (including explicitly granted failing-test repair scope)
into a Git tree and a `refs/roster/checkpoints/<task>/<n>` ref. It uses neither
stash nor human-authored commits and does not change the main index or HEAD.
Ignored, protected metadata lives under `.roster/checkpoints/<issue>/<n>`.
Secrets, managed artifacts, symlinks and hard links are not captured as products.
Standalone filesystem-only seat fixtures have no Git checkpoint capability.

Plan mode never offers tests or publication and cannot write TASK.md, RECIPE.yml
or product files before acceptance. Exploration excludes secrets and private
Roster artifacts. Acceptance preserves PLAN.md and does not widen the human
Ask's allowed-file scope. `/retry` is not implicit plan acceptance; use Enter.
CLI `roster run --issue N --plan` returns the PLAN-only handoff.

Up/Down recall entered commands without executing them until Enter. History
keeps the last 200 safe lines in ignored `.roster/history`, written atomically
with owner-only permissions. Vault commands and their hidden input, PEM paths,
credential-like values, passwords, and known secret environment values are
excluded from both saved and readline history. Consumer repositories must
ignore `.roster/history` (and its atomic temporary files) before saving history.
History is a protected harness artifact, not a coder-readable source file.

Tab completes slash commands from the shared registry; a second Tab lists
matches. Ctrl+C aborts the active seat/model request/test process and returns
to the prompt. A second Ctrl+C or Ctrl+C while idle exits with status 130.
Ctrl+D on an empty input, `/quit`, `/q`, and `exit` exit with status 0. A
cancelled task cannot be published as completed work. `/redraw` repaints the
rail without clearing scrollback; `/clear` clears the screen and repaints it.
No extra terminal dependency is used.

Unknown commands print `Unknown command. /help lists commands.` and leave the
prompt open without running a seat. Every registered command has a help page;
the same registry drives Tab completion. Secrets are never
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
automatically. Use `/run N --confirm` to stop after the summary, then press
Enter to continue in the same worktree or `/stop` to cancel. CLI
`roster run --issue N --confirm` still returns a prepared handoff and has the same
pause; it cannot be combined with `--publish`.
Multiple Outcomes now take the feature PLAN path instead.
A planning-only handoff or clarification cannot publish, even with a
review bypass. No file scope is invented for an Ask that names no files.

`/run N` produces one shell line per action event, independently of its final
summary. The tray is the only writer: the run log keeps every technical record
in `.roster/runs/`, and the shell transcript prints the phase, the tool, the
waiting tick and the verdict exactly once each. A phase line is `#108 draft`, a
tool line is `write_file README.md` and collapses on repeat into
`write_file README.md · 2`, a waiting tick rewrites its own row, and a verdict
is one line such as `108 draft · fail · length · no write`. The run log no
longer narrates the same events in sentences, so nothing is printed twice.
These are status projections only: they do not add tools, checks, edits, or work.

Unsupported LLM finish reasons are named in the shell and run log without
printing response bodies. `stop` and `tool_calls` are accepted. The first
`length` uses one extra model turn. A docs slice logs
`Response truncated. Continuing the same message.`, keeps reasoning disabled
and keeps the same completion cap of at least 8192. Other tasks retain
`Response truncated. Retrying.` with half the cap. Repeated
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
`/log debug` request with debug enabled displays a validated tail. Tail reading refuses malformed
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

After more than 30s, each waiting event rewrites the transcript waiting row in
place instead of appending a sentence. Timeout prints
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

`/model MODEL` and `/effort h` change the in-memory session, not files.
`/model MODEL --save` atomically changes only the private model field, preserving
saved endpoint settings and comments. Session fleet endpoints are not persisted
as a side effect. `/effort` sets an in-memory `llm.effort_override` so a human
choice is not lost to automatic selection. `/effort x` sends max to local DeepSeek-V4.1 and xhigh to cloud
except for docs slices, which cap all choices at high;
`/effort none` disables thinking. Remove the override field in private config
to return to difficulty/model-prior defaults in a later process. The
updated values apply to the next `/run` in this shell. With no selected
endpoint or profile, setting a model alone still leaves the stub active.

Every ask uses difficulty versus the [model capability prior](CAPABILITIES.md):
strong models use low at difficulty1-2, while difficulty4-5 uses high.
This is independent of task class or filenames. Low-difficulty docs slices
retain a `max_tokens` floor of 8192; feature/initiative plans retain 4096.
Every slice denies reads outside TASK.md and its allowed files, including
tests/fixtures and harness sources unless allowed explicitly.
A new `/run` after a failing REVIEW
uses the preceding journaled coder effort raised one supported tier (never
past max/xhigh, or high for docs slices), unless an explicit override exists. The task and minimum
context are unchanged. No retry/model call is scheduled just by selecting
effort, and no `reasoning_content` is persisted in seat memory.
`/model clear` leaves only the session model empty and opts the next run into
fleet routing; no eligible profile remains a stub, and an absent catalog needs
onboarding. `/run N --auto-model` explicitly
selects a registered fleet endpoint/model in memory, even if a saved
default exists. It prefers at least three qualifying human evaluations,
otherwise starting priors/class hints; the [routing guide](ROUTING.md)
defines the bounds. Without the flag, an empty model with a configured
endpoint is still an explicit error.
Running `/model` or `/effort` without a value shows the current setting.
Do not put API key values in the config; it remains ignored by Git.
When [onboarding](ONBOARDING.md) saved a project private config, the shell
loads it; only `/model ID --save` or an explicit probe `--set-model` changes
the saved model. Plain model/effort/fleet selection never writes that file.

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
