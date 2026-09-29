# Builtin sequential-seat SDLC

The roster is the planner and executor. The required
[`github-agent-contracts`](../vendor/github-agent-contracts) submodule is only
the GitHub publish SDK; no external coding harness, Kanban DB, or runtime npm
dependency is needed. Use Node 20+ ESM.

## Configuration

Copy [`roster.config.example.yml`](../roster.config.example.yml) to
`.roster/config.yml` in this roster checkout. That private config, asks, memory,
and issue worktrees are ignored by Git. If the private config is absent, the
tracked example is loaded. Config is a strict version 1 YAML subset: unknown,
duplicate, missing, or malformed fields fail instead of silently defaulting.
Paths are relative to this checkout (the worktrees path is used in the issue's
Git repository). Ignore a custom worktrees path yourself if you change it.

- `llm.base_url`: OpenAI-compatible chat completions base (for example
  `http://localhost:1234/v1`). Empty means a network-free deterministic stub.
  Set `llm.model` when the URL is nonempty. Local endpoints may work without
  a key; when needed, set the environment variable named by `llm.api_key_env`
  or store an API key under that name in the file vault. A non-empty environment
  value wins. Put only its **name** in config, never the key. HTTP errors report status,
  not the response body or Authorization header.
- `llm.effort` (`l|m|h|x`) and `llm.context_max` (zero means unknown) describe
  provenance for AI-Run. They are not guessed from the endpoint or sent as a
  model-specific reasoning parameter.
- `planner.turn_budget` bounds planning chat responses to 1-64 (example: 2).
  The planner has no file tools and writes only validated plan artifacts.
  Private schema 1 configs predating the planner section use a one-turn planner.
- `seat.id` and `seat.principal` are both `coder`. The principal does not confer
  merge or deploy rights. `seat.turn_budget` limits coder responses to 1-64
  (example: 8). `seat.tools` can only name the four builtin tools.
- `paths.memory` selects the coder's append-only JSONL file; the planner uses
  `planner.jsonl` beside it. Each seat loads only its own last 20 entries.
  `paths.skills`
  loads immediate `*/SKILL.md` files from **this** checkout (including
  `implement-task` and `run-tests` when present). A missing or empty skills
  directory is allowed. `paths.asks` holds local drafts;
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
capabilities.

```sh
node src/cli.mjs ask "Add a Status section to README.md"
```

When `gh` is installed, this creates an issue in the current GitHub origin
using the first Ask line as its title and the Ask as its body, then prints
the issue URL. It does not plan locally first. A failed authenticated issue
creation is an error, not an offline fallback. When `gh` is missing, it
writes `.roster/asks/<id>.md` and adjacent `RECIPE.yml` and `TASK.md`, then
prints a `gh issue create --body-file` command for later use. The offline
draft uses the deterministic stub even if an LLM endpoint is configured:
it makes no network request. A local draft uses `ask: local:<id>`; a real
issue run generates `ask: issue:N`. Run the printed command from the same
Git repository; its body file path is absolute. The stub uses the first ask
line as the title, picks up
explicit **Acceptance checks** and **Files allowed** bullet sections when
present, and otherwise lists `node --test exits 0` plus the Ask, with
referenced filenames or `**/*` subject to the tool denylist. Review broad
draft scopes before running a real issue. With an LLM, only a configured
`base_url` triggers a chat request; the model proposes a title (the issue title
takes precedence), short acceptance checks, and allowed worktree paths, which
are validated before writing the task.

[`TASK.md`](../templates/sdlc/TASK.md) puts acceptance checks and files
allowed before the Ask text. The coder must meet the checks; the runner
also enforces a final successful `node --test` in LLM mode. A failing check
must be reported, not treated as success.

## Execute and publish

```sh
node src/cli.mjs run --issue 42 --runtime builtin
```

Use `node src/cli.mjs status --issue 42` to read the issue, its open
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
task/result files. Directory listings hide protected entries. `run_test` runs
`node --test` in the worktree with a 60-second timeout and without the model
API key or App credentials. A nonzero exit returns captured stdout, stderr,
and exit code to the coder as a failed tool result so it can correct the task
within its turn budget; a timeout reports an explicit error. Final nonzero
verification fails the run and records failed memory instead of claiming
success. A successful run records `RESULT.md` and appends memory. See the
[four-tool contract and denylist](TOOLS.md) for the exact inputs and
protected surfaces.

With no endpoint, the stub writes a deterministic `RESULT.md` summary, exits
zero, and **does not edit code or run tests**. It cannot deliver a software
change; configure an LLM to do that. The command prints, but does not execute,
the publishing command:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "feat: issue 42

Closes #42" --merge-when-green
```

Run it from the **issue worktree root** after reviewing code and initializing
the pinned submodule there (`git submodule update --init --recursive`) if
needed. `--publish` executes it only after an LLM run and passing tests, with
`GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` set. It stages only changed,
task-allowed files, excluding generated task files and refusing policy,
workflows, secrets, or other out-of-scope changes. The SDK enforces the
human-owned policy, waits for required checks, then marks the App PR ready
and merges only when reviewed policy and repository rules permit it. No
deploy is requested. For an issue run, the PR body links `Closes #N`. With
builtin `--publish` or REPL `/publish`, once the SDK confirms a merge,
Roster verifies the PR, comments on the issue
with the coder's AI-Run line, and closes it through an issue-scoped App token.
If post-merge issue operations fail, the merged PR remains merged and the
error is reported; do not publish a duplicate commit to retry. The runner
prints one AI-Run for each seat with its own
session and reported token counts; the single code commit published through
the SDK carries the coder's run. Unknown slots remain unset or `-`. An API key
is not forwarded to tests or the publisher.

For a human TTY, bare `roster` opens the [interactive shell](REPL.md).
Its `/publish` command imports the App SDK in-process; agent/CI `--publish`
invokes the same SDK with `--merge-when-green`.

The earlier `roster run --issue N` remains a prepare-only compatibility
command. See [the one-task loop](ONE_TASK_LOOP.md),
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
recipe and task and loads these skills; the legacy prepare-only
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
node src/cli.mjs recipe validate RECIPE.yml
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
evidence, and remaining risks. Publication requires the pinned `v0.2.0`
contracts dependency and the human-owned policy described in the
[dependency guide](DEPENDENCY.md). Contracts owns identity, policy, and trailers;
neither the recipe nor the skills grant permissions.

With `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` set, publish from the
assigned worktree's repository root:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "<type>: issue N" --merge-when-green
```

Use a message matching the actual change. Never fall back to committing as the
signed-in human when App env is set. Never commit `.env`, tokens, or private
keys, copy contracts source, or edit policy to unblock publication. If App env
is absent, return the local handoff; if publishing prerequisites fail, report
the blocker without claiming a PR was opened.

Repository review and branch protections control whether the SDK can merge;
the human posts `AI-Eval:` on the PR. The coder does not call `git merge`,
deploy, open extra issues, or fabricate a human evaluation.
