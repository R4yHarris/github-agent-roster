# Prompt 01 — one-task loop (core)

Stay in this workspace.

Implement `roster run --issue N` (GitHub issue in the *current* git remote):

1. Read the issue body as the Ask.
2. Create a git worktree under .worktrees/issue-N (gitignore .worktrees).
3. Write .worktrees/issue-N/ASSIGNMENT.md with issue URL, number, title, body.
4. Export AI_TASK=issue-N and AI_SESSION=roster-<timestamp> into a dotenv file there.
5. Print the exact next command for a worker:
   `node $GITHUB_AGENT_CONTRACTS/scripts/agent-pr.mjs --message "feat: issue N"`
   after the worker edits inside the worktree.

Do not spawn Hermes. Do not merge. Do not open extra issues.
Tests: mock worktree path creation without network if possible; document gh issue view as required for the live command.

Default deny merge. Coder seat only.
