# GitHub is the board

GitHub Issues are the task queue; pull requests are the forge and review
trail. Roster does not create or consult `kanban.db`, SQLite task tables, or
a second scheduling service. An issue number identifies one human Ask.

1. `roster ask "..."` creates an issue in the current repository when `gh`
   is available. Without `gh`, it writes an ignored `.roster/asks/<id>.md`
   draft and prints a `gh issue create` command. The draft is **not** queued
   until a human creates the issue.
2. `roster run --issue N --runtime builtin` reads that issue, creates one
   `issue-N` worktree, and runs planner then coder in sequence. Its local
   `ASSIGNMENT.md`, `RECIPE.yml`, `TASK.md`, and `RESULT.md` describe execution,
   not board state. An empty LLM endpoint produces only a stub result.
3. Explicit publication uses the pinned contracts SDK and the GitHub App.
   Required checks and repository protections govern `--merge-when-green`.
   An issue-run PR includes `Closes #N`. Roster-managed publication verifies
   the merged PR, posts an App-authored issue comment with the coder's AI-Run
   when present, and closes the issue if GitHub has not already done so.
   A failure is reported; it is not treated as a successful board transition.
4. A human reviews and posts `AI-Eval:` on the PR. The coder never invents
   that evaluation or opens extra issues as a substitute for the Ask.

`roster status --issue N` reads the issue and its open `issue-N` branch PR
from GitHub and reports the local worktree path. `--offline` reads only the
cached assignment and filesystem: it reports PR state as **unknown**, not
`none`. The interactive shell offers the same behavior via `/status`.

Ignored `.roster/memory/<seat>.jsonl` files are seat context; optional
`.roster/runs/*.jsonl` and `.roster/evals.jsonl` are learning evidence.
Worktrees and local Ask drafts are working material. None replaces the
GitHub issue/PR queue. See [same-session seats](MULTIAGENT.md),
[the issue loop](ONE_TASK_LOOP.md), [status in the shell](REPL.md), and
[learning](LEARNING.md).
