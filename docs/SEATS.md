# Seats and recipes (v0)

A recipe describes a task's ordered seats; it does not assign credentials or
grant GitHub permissions. GitHub issues and PRs remain the queue. The builtin
planner emits two ordered seats for a single issue run:

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
```

`roster ask` makes an offline draft with `ask: local:<id>` instead; the issue
runner generates `ask: issue:N` when the human supplies an existing issue.
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
preceded by one `planner` for older recipes; builtin execution requires both.
Each seat requires `id`,
`principal`, and `worker`; keys can appear in any order. The principal is
always `coder`. Both builtin seats require their exact fixed `sequence` above.
The planner cannot call tools and writes only `RECIPE.yml` and `TASK.md`; the
coder cannot rewrite those files. Older single-coder builtin recipes and
`copilot`/`hermes` labels remain valid for read-only validation, without being
selected for execution. Validation alone does not start any worker.

There is no reviewer, merger, or deploy seat, no deploy or merge capability, and no arbitrary role or capability field. In particular, `planner` with `principal: coder` does **not** inherit merge permission. The recipe confers no permissions at all: a trusted executor must separately use the human-owned `agent-policy.yml` and GitHub App/repository protections from the sibling contracts pack before taking action. Missing or invalid recipe data fails closed; nothing falls back to an example or grants a capability.

Only a small YAML subset is supported: plain unquoted values, root keys at
column zero, list items indented two spaces, and remaining seat keys indented
four spaces. The two builtin sequences are the supported inline lists. Blank
lines, CRLF, and full-line or whitespace-separated `#` comments work. Tabs,
unknown or duplicate keys and seat IDs, omitted keys, empty lists, extra
seats, other inline collections, quoted values, anchors, aliases, tags,
document markers, and other YAML syntax are errors. Input must be UTF-8 and
at most 64 KiB; file validation requires a regular file, not a symlink.

The read-only CLI command is `roster recipe validate PATH`. Code can import
`parseRecipe(source)` to validate text, or `validateRecipe(path)` to read and
validate a file, from [the recipe module](../src/lib/recipe.mjs). Both return
the same frozen `{ version, ask, seats }` object or throw `RecipeError`.
Validation performs no network or GitHub action. See [same-session seats](MULTIAGENT.md)
and [SDLC](SDLC.md) for task files, runtime, and publication.
