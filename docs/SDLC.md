# Builtin single-seat SDLC

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
  a key; if needed, set the environment variable named by `llm.api_key_env`.
  Put only its **name** in config, never the key. HTTP errors report status,
  not the response body or Authorization header.
- `llm.effort` (`l|m|h|x`) and `llm.context_max` (zero means unknown) describe
  provenance for AI-Run. They are not guessed from the endpoint or sent as a
  model-specific reasoning parameter.
- `seat.id` and `seat.principal` are both `coder`. The principal does not confer
  merge or deploy rights. `turn_budget` limits chat responses to 1-64; the
  example uses 8. `tools` can only name the four builtin tools.
- `paths.memory` is JSONL (last 20 entries enter context); `paths.skills`
  loads `*/SKILL.md` from **this** checkout; `paths.asks` holds local drafts;
  `paths.worktrees` selects a path inside the issue repository.

## Task files and planning

[`templates/sdlc/`](../templates/sdlc) contains
[`ASK.md`](../templates/sdlc/ASK.md),
[`RECIPE.yml`](../templates/sdlc/RECIPE.yml),
[`TASK.md`](../templates/sdlc/TASK.md), and
[`ASSIGNMENT.md`](../templates/sdlc/ASSIGNMENT.md).
The generated recipe has one `coder` seat with `principal: coder`,
`worker: builtin`, and the fixed sequence
`[load_context, implement, run_tests, summarize]`. It assigns work, not GitHub
capabilities.

```sh
node src/cli.mjs ask "Add a Status section to README.md"
```

This writes `.roster/asks/<id>.md` and an adjacent
`.roster/asks/<id>/RECIPE.yml` and `TASK.md`, printing both plan paths. It
does not create an issue or contact a model when `base_url` is empty. A local
draft uses `ask: local:<id>`; a real issue run generates `ask: issue:N`.
Without an LLM, the stub uses the first ask line as the title, picks up
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
node src/cli.mjs run --issue 42 --seat coder --runtime builtin
```

The command uses the current Git repository's GitHub origin, authenticated
`gh issue view`, and branch `issue-42` in `.worktrees/issue-42`. It writes
`ASSIGNMENT.md`, `RECIPE.yml`, `TASK.md`, and an ignored `.env` containing
`AI_TASK` and `AI_SESSION`. The coder reads that worktree's `AGENTS.md` and
`TASK.md`, this roster's skills, and the last 20 memory JSONL lines. Its
`read_file`, `write_file`, and `list_dir` tools stay inside the worktree and
reject symlink escapes; writes must match the TASK file's allowed list and
cannot touch `.env*`, `*.pem`, `.git`, `agent-policy.yml`,
`.github/workflows`, or the generated task/result files. `run_test` runs
`node --test` in the worktree with a 60-second timeout and without the model
API key or App credentials. A successful run records `RESULT.md` and appends
memory. Turn exhaustion or failed tests return an error and do not publish.

With no endpoint, the stub writes a deterministic `RESULT.md` summary, exits
zero, and **does not edit code or run tests**. It cannot deliver a software
change; configure an LLM to do that. The command prints, but does not execute,
the publishing command:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "feat: issue 42"
```

Run it from the **issue worktree root** after reviewing code and initializing
the pinned submodule there (`git submodule update --init --recursive`) if
needed. `--publish` executes it only after an LLM run and passing tests, with
`GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` set. It stages only changed,
task-allowed files, excluding generated task files and refusing policy,
workflows, secrets, or other out-of-scope changes. The SDK enforces the
human-owned policy and creates a draft PR with the App identity; no merge or
deploy is requested. The runner supplies an AI-Run line and known `AI_*`
values from config and reported planner/coder tokens, leaving unknown slots
unset or `-`. An API key is not forwarded to tests or the publisher.

The earlier `roster run --issue N` remains a prepare-only compatibility
command. See [the one-task loop](ONE_TASK_LOOP.md),
[seats and recipes](SEATS.md), and [local metrics](METRICS.md).
