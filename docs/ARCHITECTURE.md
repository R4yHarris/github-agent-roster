# Architecture

```
Ask (human issue)
  → Recipe (planner): sequence of seats
  → Seat (principal): coder | reviewer | planner
       Principal = GitHub App + agent-policy role
       Capabilities = Memory, Skills, Context, Process, Tools (MCP)
  → Worker process (Hermes | Claude Code | Copilot)
       worktree, AI_* env
  → Publish: contracts agent-pr.mjs
  → Evidence: AI-Run trailer + check-agent-trailers
  → Eval: human AI-Eval comment
  → Router (later): pick model/effort from JSONL
```

SQLite Kanban is out of scope. GitHub is durable state.
