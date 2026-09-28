# Seats and recipes (v0)

A recipe describes a single issue's ordered seats; it does not assign credentials or grant GitHub permissions. GitHub issues and PRs remain the queue. Version 1 uses this exact shape:

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

All three root fields are required. `ask` is `issue:N` for a positive, safely representable decimal issue number in the current repository (no leading zeroes). `seats` contains exactly one `coder`, optionally preceded by one `planner`. Each seat requires exactly `id`, `principal`, and `worker`; keys can appear in any order. The only v0 principal is `coder`, and the only worker names are `copilot` and `hermes`. A worker name is descriptive: validation does not start a worker.

There is no reviewer, merger, or deploy seat, no deploy or merge capability, and no arbitrary role or capability field. In particular, `planner` with `principal: coder` does **not** inherit merge permission. The recipe confers no permissions at all: a trusted executor must separately use the human-owned `agent-policy.yml` and GitHub App/repository protections from the sibling contracts pack before taking action. Missing or invalid recipe data fails closed; nothing falls back to an example or grants a capability.

Only a small YAML subset is supported: plain unquoted values, root keys at column zero, list items indented two spaces, and remaining seat keys indented four spaces. Blank lines, CRLF, and full-line or whitespace-separated `#` comments work. Tabs, unknown or duplicate keys and seat IDs, omitted keys, empty lists, extra seats, inline collections, quoted values, anchors, aliases, tags, document markers, and other YAML syntax are errors. Input must be UTF-8 and at most 64 KiB; file validation requires a regular file, not a symlink.

The read-only CLI command is `roster recipe validate PATH`. Code can import `parseRecipe(source)` to validate text, or `validateRecipe(path)` to read and validate a file, from [the recipe module](../src/lib/recipe.mjs). Both return the same frozen `{ version, ask, seats }` object or throw `RecipeError`. Validation performs no network or GitHub action.
