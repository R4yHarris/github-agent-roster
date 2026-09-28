# Architecture

```
Ask (human GitHub issue; optional local draft)
  → Recipe + TASK (stub or configured LLM planner): one builtin coder
  → Coder seat (principal: coder; no merge/deploy)
       AGENTS.md, TASK.md, last 20 memory lines, this repo's skills
       bounded chat loop with read_file, write_file, list_dir, run_test
       issue worktree, RESULT.md, AI_* provenance
  → Optional publish: contracts agent-pr.mjs (GitHub App, human-owned policy)
  → Evidence: AI-Run trailer + check-agent-trailers
  → Eval: human AI-Eval comment
```

An empty LLM endpoint selects a deterministic offline stub that writes a
RESULT summary but does not implement code. The contracts submodule is a
publish SDK, not a worker. The earlier prepare-only and external-worker
recipes remain parseable for compatibility; no external harness is required.
GitHub is durable state, and SQLite Kanban is out of scope. See [SDLC](SDLC.md).
