# Seats and recipes (v0)

A recipe describes a task's ordered seats; it does not assign credentials or
grant GitHub permissions. GitHub issues and PRs remain the queue. The builtin
planner emits three ordered seats for a single issue run:

```yaml
version: 1
ask: issue:42
seats:
  - id: planner
    principal: coder
    worker: builtin
    sequence: [read_ask, plan, write_task]
  - id: coder
    principal: coder
    worker: builtin
    sequence: [load_context, implement, run_tests, summarize]
  - id: reviewer
    principal: reviewer
    worker: builtin
    sequence: [read_diff, check_acceptance, write_review]
```

When `gh` is unavailable, `roster ask` makes an offline draft with
`ask: local:<id>`; when available it creates an issue. The issue runner
generates `ask: issue:N` from an existing or newly created issue.
The strict parser also accepts the previous version 1 prepare-only shape for
compatibility:

```yaml
version: 1
ask: issue:42
seats:
  - id: planner
    principal: coder
    worker: copilot
  - id: coder
    principal: coder
    worker: hermes
```

All three root fields are required. `issue:N` uses a positive, safely
representable decimal issue number (no leading zeroes); `local:<id>` uses an
opaque local draft identifier. `seats` contains exactly one `coder`, optionally
preceded by one `planner` for older recipes; the issue runner requires
all three ordered builtin seats.
Each seat requires `id`,
`principal`, and `worker`; keys can appear in any order. Planner and coder
use principal `coder`; reviewer uses the fixed read-only `reviewer` principal
and the builtin worker only. Each builtin seat requires its exact fixed
`sequence` above. Legacy recipes without the reviewer remain valid for
read-only parsing, not for the new issue execution path.
The worktree planner may call only artifact-scoped `write_file` for root
`RECIPE.yml`, `TASK.md`, and `ESTIMATE.md`; app-code paths are denied. The harness
validates and finalizes those managed outputs. The coder cannot rewrite those
files or the harness-owned REVIEW.md. Older single-coder builtin recipes and
`copilot`/`hermes` labels remain valid for read-only validation, without being
selected for execution. Validation alone does not start any worker.

The reviewer inspects the task checks, diff, and RESULT.md and writes
REVIEW.md through the harness, not through a model file tool. A human still
reviews PRs and records AI-Eval. There is no merger or deploy seat, no deploy or merge
capability, and no arbitrary role or capability field. In particular, `planner`
with `principal: coder` does **not** inherit merge permission. The recipe
confers no permissions at all: a trusted executor must separately use the
human-owned `agent-policy.yml` and GitHub App/repository protections from the
contracts pack before taking action. Missing or invalid recipe data fails
closed; nothing falls back to an example or grants a capability.

### Optional seat capability bounds (FEATURE_SPEC.md section 5.2)

Each seat may declare `skills: [implement-task, run-tests]`,
`tools: [read_file, write_file, edit_file, run_test]`, and `max_difficulty: 3`.
The difficulty ceiling is an integer from 1 to 5. Skills accept the packet
names `implement-task`, `read-before-write`, `result-report`, `run-tests`, and
`small-diff`, plus the spec categories `research`, `implement`, `test`, `review`,
`incident`, and `docs`. Categories describe coverage; selecting a category in
TASK.md still requires a corresponding installed skill packet. Tools accept
only the runtime's named tools: `read_file`, `write_file`, `edit_file`,
`glob_files`, `run_command`, `list_dir`, `run_test`, `search_text`, `web_search`,
and `web_fetch`. Lists must be inline, unquoted, distinct known names.

Before starting the recipe's coder, the harness checks every TASK.md
frontmatter skill and its validated difficulty (default 2) against declared
bounds. `assertSeatCovers(seat, { skills, difficulty })` performs this pure
check and throws `RecipeError` on a mismatch. Omitted bounds retain legacy
behavior. Explicit `skills: []` covers only tasks with no selected skills;
explicit `tools: []` denies every tool, including automatically offered ones
(section 5.1). A declared tools list only restricts existing runtime access;
it never enables internet or grants GitHub permissions. Unsupported values
fail closed. These bounds do not add a queue or concurrent workers.

Only a small YAML subset is supported: plain unquoted values, root keys at
column zero, list items indented two spaces, and remaining seat keys indented
four spaces. The builtin sequences and optional capability lists are the supported inline lists. Blank
lines, CRLF, and full-line or whitespace-separated `#` comments work. Tabs,
unknown or duplicate keys and seat IDs, omitted required keys, empty seat lists, extra
seats, other inline collections, quoted values, anchors, aliases, tags,
document markers, and other YAML syntax are errors. Input must be UTF-8 and
at most 64 KiB; file validation requires a regular file, not a symlink.

The read-only CLI command is `roster recipe validate PATH`. Code can import
`parseRecipe(source)` to validate text, or `validateRecipe(path)` to read and
validate a file, from [the recipe module](../src/lib/recipe.mjs). Both return
the same frozen `{ version, ask, seats }` object or throw `RecipeError`.
Validation performs no network or GitHub action. See [same-session seats](MULTIAGENT.md)
and [SDLC](SDLC.md) for task files, runtime, and publication.
