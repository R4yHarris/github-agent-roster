# Prompt 02 — seats and recipes

Add docs/SEATS.md and src/lib/recipe.mjs.

A recipe is YAML with no extra keys:

```yaml
version: 1
ask: issue:N
seats:
  - id: planner
    principal: coder
    worker: copilot
  - id: coder
    principal: coder
    worker: hermes
```

Parse strictly like contracts policy (small YAML subset). Unknown keys fail.
`roster recipe validate PATH`.
Planner does not get merge. No deploy seat in v0.
Tests for valid/invalid recipes.
