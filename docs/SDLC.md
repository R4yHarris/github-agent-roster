# Builtin sequential-seat SDLC

The roster is the planner and executor. The required
[`github-agent-contracts`](../vendor/github-agent-contracts) submodule is only
the GitHub publish SDK; no external coding harness, Kanban DB, or runtime npm
dependency is needed. Use Node 20+ ESM.

## Agile mapping

GitHub issues and PRs are the board and forge. Before any seat runs, the
Ask is classified independently of `task_class` (`feat|fix|docs|test`) and
difficulty:

| Ask kind | Execution |
| --- | --- |
| `clarify` | Stop with a concrete outcome/file-scope clarification. No model seat, implementation, or publication. |
| `slice` | One bounded outcome uses the minimum planner/coder/read-only-reviewer loop. A valid cached TASK/recipe skips planner. |
| `feature` | Planner writes `PLAN.md` with **2-5 child issue drafts**, each with planned file scope. An issue run opens them as linked GitHub child issues and continues into the first runnable wave slice. |
| `initiative` | Planner writes `PLAN.md` only: outcomes, waves, and child issue drafts. No coder or application edits. |

Classification is deterministic and conservative, not an extra model call:
a README one-liner or named-file task is a slice; multiple explicit `Outcomes`
or an explicit feature/end-to-end request is a feature; "build an orchestrator",
more than five declared outcomes, or a whole-system/multi-wave initiative is
an initiative. Merely mentioning
these words in documentation or acceptance checks does not change the kind.
Missing executable scope or unclear planning intent yields `clarify`, never
an invented file allowlist. A validated existing TASK can supply slice scope,
but cannot turn a feature or initiative into a coder run.

```text
Ask -> classify
  clarify -> human clarification
  slice -> RECIPE.yml + TASK.md -> coder -> reviewer -> reviewed App PR -> human AI-Eval
  feature -> planner -> PLAN.md -> linked wave:N child issues -> first runnable slice
  initiative -> planner -> PLAN.md -> human review of feature drafts
```

Plans are bounded Markdown drafts, not another task board. Each draft has a
title, one outcome, acceptance checks, and an issue label such as `wave:1`;
the Waves section groups those drafts by label. File scope comes from
human-named paths. When the human named none, the feature planner proposes
each draft's planned files from the tracked repository paths (never `**/*`,
protected, or vendor paths), and PLAN.md records `Scope: planner-proposed`.
The planner also receives `existing_exports`: up to 12 tracked source modules whose
paths or exported names share terms with the Ask, so drafts reuse resolvers such as
the machine root instead of inventing parallel modules or hardcoded paths.
A rejected PLAN is returned to the model with its validation error, up to four
bounded attempts.

For `roster run --issue N`, an executable feature PLAN is delivered rather than
parked: Roster opens the missing drafts as GitHub issues labelled `wave:N`,
each body naming `Parent: #N` and the PLAN markers, then runs the first `todo`
child as an ordinary slice in its own worktree. Later waves stay blocked until
earlier wave issues close. Rerunning the parent reuses its PLAN, so child
issues are never duplicated, and continues with the next ready child. Use
`--confirm` to stop at the PLAN instead. Local Asks and initiatives still stop
at PLAN.md; initiative drafts are features that are planned on their own runs.
Neither `--publish` nor a review bypass can publish planning-only output.

The same classification applies to offline Ask drafts, Ask-file demos, and
standalone TASK execution. Empty endpoints remain deterministic stubs.
Prior generated PLAN/task/run files are archived when reusing an issue
worktree; application edits and assignment/environment are preserved.

## Configuration

Copy [`roster.config.example.yml`](../roster.config.example.yml) to
`.roster/config.yml` in this roster checkout. That private config, asks, memory,
and issue worktrees are ignored by Git. If the private config is absent, the
tracked example is loaded. Config is a strict version 1 YAML subset: unknown,
duplicate, missing, or malformed fields fail instead of silently defaulting.
Paths are relative to this checkout (the worktrees path is used in the issue's
Git repository). Ignore a custom worktrees path yourself if you change it.
The human [shell](REPL.md) can atomically persist `/model` and `/effort`
without editing the tracked example or storing credentials.

- `llm.base_url`: OpenAI-compatible chat completions base (for example
  `http://127.0.0.1:8000/v1`). Empty with no selected `llm.profile` means a
  network-free deterministic stub. Select `vllm-local`, `ollama`, `lmstudio`,
  or `openai` from `profiles` instead of setting a custom URL. Then set
  `llm.model` or explicitly use `--auto-model` with an empty model. With fewer than three
  matching human evaluations, auto-model runs the deterministic stub; see
  [routing](ROUTING.md).
  Local endpoints may work without
  a key; when needed, set the environment variable named by `llm.api_key_env`
  or store an API key under that name in the file vault. A non-empty environment
  value wins. Put only its **name** in config, never the key. HTTP errors report status,
  not the response body or Authorization header.
- `llm.effort` (`l|m|h|x|none`) describes effort; automatic seat selection uses
  task difficulty versus the model capability prior for every ask. A strong
  model uses low at difficulty1-2; difficulty4-5 uses high, regardless of
  task class or filenames. Docs difficulty1-2 retains at least 8192 output tokens;
  feature/initiative plans retain 4096; slice context remains minimum. Requests send the
  selected `reasoning_effort` and `max_tokens`. Cloud maps l/m/h/x to
  low/medium/high/xhigh. Local DeepSeek-V4.1 maps to low/high/high/max;
  `none` disables thinking (local DeepSeek template `thinking: false`).
  `/effort` persists `llm.effort_override`, which wins over automatic and retry
  selection except that docs slices never exceed high. Remove that optional field from private config to resume automatic
  selection. A retry of a failed review raises the last recorded coder effort
  one supported tier, capped at high for docs slices and otherwise the backend maximum. No automatic retry or
  extra seat is created. `llm.context_max` remains declared capacity, not usage.
- Optional `llm.request_timeout_ms` overrides the complete HTTP request
  deadline with a positive integer, at most 2147483647 milliseconds. Without
  it, local loopback/private-IP hosts (`127.*`, `localhost`, `::1`,
  `192.168.*`, `10.*`, and `172.16-31.*`) use **20 minutes**; cloud/public
  hosts use **120 seconds**. This follows the selected endpoint, including
  fleet routing, not a profile name. See [cold starts](REPL.md#local-llm-cold-starts).
  A fleet profile can override `request_timeout_ms` for a public cold-inference
  gateway. The slice planner retries one shorter-deadline timeout, visibly,
  without changing the model or replaying completed tools; a full 20-minute
  timeout is terminal.
- `planner.turn_budget` bounds planning chat responses to 1-64 (example: 2).
  The slice planner has only artifact-scoped `write_file` for root
  `RECIPE.yml`, `TASK.md`, and `ESTIMATE.md`; no app-code tools. The harness
  validates and finalizes those files. Tool replies and repairs consume turns.
  Prefer one final JSON object with `title`, `acceptance_checks`, and
  `files_allowed`; the harness can also normalize `task` as a title alias and
  accept `steps`/`notes` hints without granting extra scope. A complete validated
  JSON plan accompanying an incomplete artifact draft is finalized without an
  extra model call. Invalid drafts return their validation error to the planner,
  rather than a misleading successful write alone. Written tasks and JSON plans
  must both keep the routed model. Failed planning stops the seat chain:
  unverified stubs never start a coder, reviewer, tests, or publisher.
  Feature/initiative planning instead uses tool-free JSON and a PLAN-only
  harness writer, with at most one correction inside that budget.
  Private schema 1 configs predating the planner section use a one-turn planner.
- `seat.id` and `seat.principal` are both `coder`. The principal does not confer
  merge or deploy rights. `seat.turn_budget` limits coder responses to 1-64
  (example: 8). `seat.tools` can only name the five builtin tools.
  `seat.context_chars` bounds the initial pack (default 8000 characters).
- `paths.memory` selects the coder's append-only JSONL file; the planner uses
  `planner.jsonl` beside it. Each seat loads only its own last 20 entries.
  `paths.skills`
  loads only the `skills:` names selected by task frontmatter from **this**
  checkout. Missing requested skills fail; an unrequested skill is not loaded.
  `paths.asks` holds local drafts;
  `paths.worktrees` selects a path inside the issue repository.

## Task files and planning

[`templates/sdlc/`](../templates/sdlc) contains
[`ASK.md`](../templates/sdlc/ASK.md),
[`RECIPE.yml`](../templates/sdlc/RECIPE.yml),
[`TASK.md`](../templates/sdlc/TASK.md), and
[`ASSIGNMENT.md`](../templates/sdlc/ASSIGNMENT.md).
The generated recipe has builtin `planner` then `coder` seats with principal
`coder`. Their fixed sequences are `[read_ask, plan, write_task]` and
`[load_context, implement, run_tests, summarize]`. It assigns work, not GitHub
capabilities. The coder's recipe labels expand to the
[eight-stage single-seat contract](SEAT.md); they do not create additional seats.

```sh
roster ask "Add a Status section to README.md"
```

For a non-clarification Ask, when `gh` is installed, this creates an issue in the current GitHub origin
using the first Ask line as its title and the Ask as its body, then prints
the issue URL. It does not plan locally first. A failed authenticated issue
creation is an error, not an offline fallback. When `gh` is missing, it
writes `.roster/asks/<id>.md` and adjacent slice `RECIPE.yml` and `TASK.md`
(or feature/initiative `PLAN.md`), then
prints a `gh issue create --body-file` command for later use. The offline
draft uses the deterministic stub even if an LLM endpoint is configured:
it makes no network request. A local draft uses `ask: local:<id>`; a real
issue run generates `ask: issue:N`. Run the printed command from the same
Git repository; its body file path is absolute. The stub uses the first ask
line as the title, picks up
explicit **Acceptance checks** and **Files allowed** bullet sections when
present, and otherwise lists `node --test exits 0` plus the Ask, with
human-named filenames subject to the tool denylist. No wildcard is invented
for absent scope. With an LLM, only a configured
`base_url` triggers a chat request; the model proposes a title (the issue title
takes precedence), short acceptance checks, and allowed worktree paths, which
are validated before writing the task.

[`TASK.md`](../templates/sdlc/TASK.md) puts acceptance checks and files
allowed before the Ask text. The coder must meet the checks; the runner
also enforces a final successful `node --test` in LLM mode unless initial
frontmatter explicitly declares `tests: none`. A failing check
must be reported, not treated as success.

## Execute and publish

```sh
roster run --issue 42
roster run --issue 42 --auto-model
```

`--auto-model` is opt-in and requires an empty configured model. It uses
the issue title's recognized task class to select an evaluated model/effort
pair with at least three distinct samples. The selection is only in memory:
it does not rewrite `.roster/config.yml`. Insufficient data, an unknown
task class, or no configured endpoint leaves the run in stub mode.

Use `roster status --issue 42` to read the issue, its open
`issue-42` branch PR, and the local worktree path. Add `--offline` to read
only the cached assignment and filesystem; an unavailable PR is shown as
unknown, not absent. See the [interactive shell](REPL.md) for `/status`.

`roster run` uses the current Git repository's GitHub origin, authenticated
`gh issue view`, and branch `issue-42` in `.worktrees/issue-42`. It writes
`ASSIGNMENT.md`, `RECIPE.yml`, `TASK.md`, and an ignored `.env` containing
`AI_TASK` and the coder's `AI_SESSION`. The planner writes only `RECIPE.yml`
and `TASK.md`; the coder reads that worktree's `AGENTS.md`, `TASK.md`, this
roster's skills, and the last 20 memory JSONL lines. Its
`read_file`, `write_file`, and `list_dir` tools stay inside the worktree and
reject symlink escapes; writes must match the TASK file's allowed list and
cannot touch `.env*`, `*.pem`, `.git`, `agent-policy.yml`,
`.github/workflows`, the pinned contracts submodule, or the generated
task/result files. Paths with `..`, absolute paths, and `vendor/` are refused
before any tool runs (`Refused: outside the worktree.`), and a vendor path in
the diff fails review. Directory listings hide protected entries. `run_test` runs
`node --test tests/repl.test.mjs` with a 60-second cap for a README-only docs
slice, or the full `node --test` suite with a 5-minute cap otherwise, always
without the model
API key or App credentials. A nonzero exit returns captured stdout, stderr,
and exit code to the coder as a failed tool result so it can correct the task
within its turn budget; a timeout reports an explicit error. Final nonzero
verification fails the run and records failed memory instead of claiming
success. After a successful write to the sole TASK-allowed file, checks run
immediately; green checks prohibit another read or search, while failed checks
reopen scoped tools for repair. A successful run appends memory, runs the
excellence gate, and writes RESULT.md.
Failed configured runs also save a truthful result before rejecting. See the
[five-tool contract and denylist](TOOLS.md) for the exact inputs and
protected surfaces.
For any bounded one-file slice, the coder is offered only scoped reads, the
named product write, and checks—never directory listing or repository search.
A tool call is an action, not a result; complete tool calls run before the same
seat continues. The product write runs checks immediately. One failed check
returns once to draft, while a second failure stops without starting review.
Narration remains a live rewritten transcript line and is not the seat result.

After a successful RESULT.md, the read-only builtin reviewer checks the task
acceptance checks and the Git diff against that result, then writes REVIEW.md
with pass/fail reasons and security notes. A failed or productless draft does
not open review. A completed model-backed review failure returns its
per-check findings to a fresh coder context (at most two repairs, switching
perspective when the same checks stay unmet), then reviews the repaired result
again. A failed review after the last repair leaves the
coder's changes intact and blocks Roster-managed publication by default;
see [review and explicit bypass](REVIEW.md).

With no endpoint, the stub writes a deterministic `RESULT.md` summary, exits
zero, and **does not edit code or run tests**. It cannot deliver a software
change; configure an LLM to do that. The command prints, but does not execute,
the publishing command only for a configured, completed coder run:

```powershell
node vendor\github-agent-contracts\scripts\agent-pr.mjs --message "<subject plus Model, Summary, how-to-test, and Refs #42>" --model GPT-6.1-Sol --merge-when-green
```

Run it from the **issue worktree root** after reviewing code and initializing
the pinned submodule there (`git submodule update --init --recursive`) if
needed. Missing or invalid model configuration reports `set model` before
the SDK; stub runs print no runnable publication command.
Completed runs retain their actual coder model and reported usage. Without
a completed seat, GHCP publication requires `AI_MODEL` or an explicit publish
model, sets `github-copilot`, and leaves used/out unknown rather than borrowing
the configured served model. PR messages include `## Model`,
`## Summary`, and how to test, not just the subject.
`--publish` executes it only after an LLM run and passing tests, with
`GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` set. It stages only changed,
task-allowed files, excluding generated task files and refusing policy,
workflows, secrets, or other out-of-scope changes. The SDK enforces the
human-owned policy, waits for required checks, then marks the App PR ready
and merges only when reviewed policy and repository rules permit it. No
deploy is requested. A passing, unchanged REVIEW.md is required unless
`--skip-review` explicitly bypasses **only** the reviewer verdict; tests
and coder excellence must still pass. For an issue run, the PR body lists
the seats, notes any bypass, and links `Refs #N` without automatically
closing the issue. With
builtin `--publish` or REPL `/publish`, once the SDK confirms a merge,
Roster verifies the PR, then comments on the still-open issue with its URL,
the real coder model ID, and AI-Run line through an issue-scoped App token.
It never closes the issue: the human does so after posting AI-Eval.
If post-merge issue operations fail, the merged PR remains merged and the
error is reported; do not publish a duplicate commit to retry. The runner
prints one AI-Run for each model-backed planner, coder, and reviewer seat with its own
session and reported token counts; the single code commit published through
the SDK carries the coder's run. Unknown slots remain unset or `-`. An API key
is not forwarded to tests or the publisher.

### Issue run status (the board follows the run)

When `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` are set, every issue
run (CLI or REPL, including wave slices) reports itself on the issue through
the same issue-scoped App token. No flag is needed. Human-owned policy must
grant the coder `comment` and `label`. Labels are the shared signal for other
agents and humans:

| Moment | Label | Comment |
| --- | --- | --- |
| Worktree prepared | `roster:in-progress` | Run started, with branch and seats |
| Review passes | `roster:review` | Review verdict, coder model, ready to publish |
| Published and merged | `roster:review` | None (the merge comment already reports it) |
| Failure, cancellation, clarification, or human pause | `roster:blocked` | Redacted reason |

Each run swaps out the other `roster:*` labels and creates a missing label
once. If the issue already carries `roster:in-progress`, the run warns that
another agent may be working it, unless this machine already holds the issue
worktree, which it reports as a resume. The wave board treats `roster:in-progress`
as running, so a parallel run never auto-picks that slice. It treats
`roster:review` and `roster:blocked` like `review` and `blocked`. A feature
parent stays claimed while its wave slice reports on its own issue.

Status updates are best-effort: a GitHub error is logged and never fails the
run. The App never closes or reopens issues. If the installation holds
**Organization projects: write**, the issue's Projects Status moves to the
`In progress` and `In review` columns, matched by name; blocked leaves the
column unchanged. GitHub Apps cannot write user-owned Projects. For those,
the labels and comments are the status; use the project's built-in workflows
(for example, closed → Done) or move the project to an organization.

For a human TTY, bare `roster` opens the [interactive shell](REPL.md).
Its `/publish` command imports the App SDK in-process; agent/CI `--publish`
invokes the same SDK with `--merge-when-green`.

`roster prepare --issue N` retains the manual handoff without running seats.
Explicit `--runtime builtin` remains accepted on `roster run` for CI and
agents. See [the one-task loop](ONE_TASK_LOOP.md),
[same-session seats](MULTIAGENT.md), [recipes](SEATS.md), and
[local metrics](METRICS.md).

## Manual planning and acceptance checks

The v0 software loop is:

```text
Human issue (Ask) -> planner's recipe and task -> one coder worktree
                 -> tested PR through contracts -> human AI-Eval
```

GitHub issues and PRs are the queue and durable record. Local handoff documents
are not a second board or database. Planning is a preparation step; the recipe
here has exactly one execution seat, `coder`, with principal `coder`. It grants
no merge or deploy rights.

## Handoff documents

| Template | Author and purpose |
| --- | --- |
| [Manual ASK.md](../templates/sdlc/manual/ASK.md) | Human: issue body describing the problem, observable requirements, scope, and open questions. |
| [Manual RECIPE.yml](../templates/sdlc/manual/RECIPE.yml) | Planner: validated link to that issue and one builtin coder seat. Post the recipe on the issue. |
| [Manual TASK.md](../templates/sdlc/manual/TASK.md) | Planner: bounded software work, acceptance checks, and the verification plan. Share it on the same issue. |
| [Manual ASSIGNMENT.md](../templates/sdlc/manual/ASSIGNMENT.md) | Orchestrator or human: issue snapshot, existing worktree, task and recipe references, and worker context. |

These expanded forms live in `templates/sdlc/manual/` so human handoff guidance
does not replace the renderer placeholders or change generated task formats.
They are not additional CLI inputs. The builtin path above generates its own
recipe and task and loads these skills; the explicit prepare-only
[one-task loop](ONE_TASK_LOOP.md) creates an assignment and ignored `.env`
without executing a coder. If a worktree is already assigned, stay there;
do not run the loop again to create another one.

## 1. Make the ask concrete

Use the ask template as the issue body in the current repository. Describe the
caller, current behavior, desired behavior, a reproduction or example, and
non-goals. Name allowed and protected areas, compatibility constraints, and
dependency restrictions. Keep credentials and environment-file contents out.

Give requirements stable IDs such as R1, R2, and R3. "Improve validation" is not
ready for implementation: specify which input is invalid, the error the caller
should observe, and whether any file, network, or other side effect is allowed.
Resolve decisions that affect behavior on the issue before handing off coding.

## 2. Turn requirements into a task

Copy the task template to the assigned worktree root as `TASK.md`. The planner
can be a human or a planning worker; that does not add a seat or permissions to
this recipe. Link the issue and copy only the agreed scope, inputs, and decisions.

For each requirement:

1. Write an acceptance ID, such as AC-1, mapped to the source requirement.
2. Specify **Given** concrete setup and input, **When** the software operation
   runs, **Then** the observable result. Include exact output shapes, stable
   errors, and required or forbidden side effects.
3. Cover the normal path, meaningful invalid inputs or failures, boundary
   values, and behavior that must not regress. For limits, specify the threshold
   and test values on either side; for performance, specify units and measurement.
4. Name the Node test file and test case that will prove the result. New behavior
   may need a new test. If automation cannot establish a check, give a repeatable
   manual procedure and the evidence to capture instead.
5. List the exact `node --test` commands and leave evidence marked **Pending**
   until those checks actually run.

Checks describe software behavior, not implementation activities: "edit the
parser" and "tests pass" alone do not prove an ask. Every requirement must map
to a check; a green suite with no relevant assertion is not acceptance. Blocking
ambiguity goes back to the issue, not into an invented default.

### Worked example

Suppose the ask is: "Reject invalid issue numbers before creating a worktree,
while preserving the existing handoff for valid issues." A task could contain:

| Check | Given / When / Then | Automated evidence |
| --- | --- | --- |
| AC-1 (invalid input) | Given `0`, `-1`, `1.5`, or `"01"`, when `runIssue` is called, it rejects with `TypeError` and `Issue number must be a positive safe integer`; no git, gh, or file-write operation occurs. | Assert the error and empty command/write logs in the injected harness. |
| AC-2 (regression) | Given issue `42` on the current origin, when assignment succeeds, the worktree is `issue-42` under `.worktrees`, the Ask body is unchanged, and the environment has `AI_TASK=issue-42`. | Assert command arguments, assignment contents, and environment output. |
| AC-3 (failure) | Given a failed issue lookup, when assignment is attempted, the failure is reported and no worktree or handoff files are created. | Inject lookup failure and assert no worktree command or file writes. |

The existing [issue tests](../tests/issue.test.mjs) demonstrate these harness
patterns without network access. The focused command is:

```sh
node --test tests/issue.test.mjs
```

This is an example of turning an ask into checks, not a request to change the
current implementation. A task must record the actual test names and outcomes
for its own requirements rather than borrowing this example's evidence.

## 3. Prepare the one-coder recipe and assignment

Copy the recipe template as `RECIPE.yml` alongside the task. Replace its example
`ask: issue:42` with the real positive issue number. Keep exactly one seat with
`id: coder` and `principal: coder`. The template uses `worker: builtin` with
`[load_context, implement, run_tests, summarize]`. Legacy `hermes` and `copilot`
recipes remain supported by the validator, but neither worker is required for
the builtin loop. This template has no planner execution seat.

Follow the strict [recipe format](SEATS.md): do not add task paths, skills,
acceptance checks, dependencies, capabilities, merge flags, or arbitrary keys
to the YAML. That context belongs in the Markdown handoff. The existing
read-only validator can check the copied file from the repository root:

```sh
roster recipe validate RECIPE.yml
```

Use the assignment template for a manual handoff. A human can append its handoff
sections to an existing assignment without changing the captured Ask; builtin
tools cannot rewrite generated task files. Keep
`TASK.md`, `RECIPE.yml`, and `ASSIGNMENT.md` together in the assigned worktree
root so their sibling links work. Ensure the issue, recipe, task ID, and
`AI_TASK=issue-N` agree; use the supplied `AI_SESSION`, not a fabricated identity.
Replace all placeholders before implementation.

## 4. Implement and gather evidence

Give the single coder the [implement-task skill](../skills/implement-task/SKILL.md).
The coder reads the repository instructions and handoff, checks the actual code,
and makes only the permitted implementation, regression-test, and documentation
changes. Node 20+ ESM and zero new runtime dependencies remain the default.

Use the [run-tests skill](../skills/run-tests/SKILL.md), which uses only
`node --test`. The builtin coder calls `run_test`, which runs the full suite.
For a manual shell-based handoff, start with affected test files in one
invocation and broaden when required by scope. Do not install another runner.
Record exact commands, exit codes, test names and counts, and the result for
each acceptance ID. Builtin evidence belongs in the final summary and generated
`RESULT.md`, not edits to protected task files. Record manual evidence separately.

A failed, skipped, unmatched, or unrun check is not a pass. Fix in-scope failures
and rerun; report out-of-scope failures, missing prerequisites, and uncovered
checks as blockers. Do not change expectations merely to turn a failure green.
Keep the issue's agreed plan and the PR's evidence consistent with the task.

## 5. Publish, then let the human evaluate

The PR handoff should link the issue and summarize the change, acceptance
evidence, and remaining risks. Publication requires the fail-closed contracts
contracts dependency and the human-owned policy described in the
[dependency guide](DEPENDENCY.md). Contracts owns identity, policy, and trailers;
neither the recipe nor the skills grant permissions.

With `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` set, publish from the
assigned worktree's repository root:

```powershell
node vendor\github-agent-contracts\scripts\agent-pr.mjs --message "<type>: issue N plus Model, Summary, and how-to-test sections" --model GPT-6.1-Sol --merge-when-green
```

Use a message matching the actual change. Never fall back to committing as the
signed-in human when App env is set. Never commit `.env`, tokens, or private
keys, copy contracts source, or edit policy to unblock publication. If App env
is absent, return the local handoff; if publishing prerequisites fail, report
the blocker without claiming a PR was opened.

Repository review and branch protections control whether the SDK can merge;
the human posts `AI-Eval:` on the PR. The coder does not call `git merge`,
deploy, open extra issues, or fabricate a human evaluation.
