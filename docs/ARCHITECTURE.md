# Architecture

```
Ask (GitHub issue; local draft when gh is absent)
  → Planner seat (stub or configured LLM): RECIPE + TASK, no file tools
  → Coder seat (principal: coder; no merge/deploy), same process/worktree
       principal -> context pack -> research -> task-selected skills
       bounded chat loop with read_file, write_file, list_dir, run_test, search_text
       append memory -> excellence gate -> RESULT.md, AI_* provenance
  → Optional publish: contracts agent-pr.mjs (GitHub App, human-owned policy)
  → Evidence: per-seat AI-Run records, coder trailer + check-agent-trailers
  → Eval: human AI-Eval comment
```

An empty LLM endpoint selects a deterministic offline stub that writes a
RESULT summary but does not implement code. The contracts submodule is a
publish SDK, not a worker. The earlier prepare-only and external-worker
recipes remain parseable for compatibility; no external harness is required.
GitHub is durable state, and SQLite Kanban is out of scope. See the
[GitHub board](BOARD.md), [same-session seats](MULTIAGENT.md), and
[SDLC](SDLC.md).
