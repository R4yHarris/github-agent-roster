# GitHub is the board

GitHub Issues are the task queue; pull requests are the forge and review
trail. Roster does not create or consult `kanban.db`, SQLite task tables, or
a second scheduling service. An issue number identifies one human Ask.

1. `roster ask "..."` creates an issue in the current repository when `gh`
   is available. Its body preserves the Ask and includes initial
   `task_class`, `difficulty`, and `estimate_min` fields:

   ```text
   # Ask

   fix(cli): Correct the status display.

   ## Task metadata

   task_class: fix
   difficulty: 2
   estimate_min: 15
   ```

   The first-line conventional task class is used when present; otherwise
   the initial class is `feat`. Difficulty 2 and 15 minutes are a baseline,
   not a completion claim. Without `gh`, Roster writes the same issue body
   to ignored `.roster/asks/<id>.md` and prints a
   `gh issue create --body-file` command. The draft is **not** queued until
   a human creates the issue.
2. `roster run --issue N` reads that issue, creates one
   `issue-N` worktree, and runs planner, coder, then reviewer in sequence.
   The planner carries the issue's validated task metadata into TASK.md
   before any refinement; older issues without the section remain usable.
   Malformed metadata fails before worktree creation. Local `ASSIGNMENT.md`,
   `RECIPE.yml`, `TASK.md`, `RESULT.md`, and `REVIEW.md` describe execution,
   not board state. An empty LLM endpoint produces an unverified stub result
   and failing review.
3. Explicit publication uses the pinned contracts SDK and the GitHub App.
   Required checks and repository protections govern `--merge-when-green`.
   An issue-run PR uses `Refs #N`, **not** a GitHub auto-closing keyword.
   Roster verifies the merged PR and that the issue is still open, then posts
   one App-authored comment with the verified PR URL, actual model ID, and
   coder AI-Run when present. It never PATCHes the issue state. A failure
   is reported; it is not treated as a successful board transition.
4. A human reviews and posts `AI-Eval:` on the PR, then closes the issue
   when satisfied. The coder and App never invent that evaluation or close
   an issue on the human's behalf.

`roster status --issue N` reads the issue and its open `issue-N` branch PR
from GitHub and reports the local worktree path. `--offline` reads only the
cached assignment and filesystem: it reports PR state as **unknown**, not
`none`. The interactive shell offers the same behavior via `/status`.

Ignored `.roster/memory/<seat>.jsonl` files are seat context; automatic seat
`.roster/runs/*.jsonl` and human `.roster/evals.jsonl` are learning evidence.
Worktrees and local Ask drafts are working material. None replaces the
GitHub issue/PR queue. See [same-session seats](MULTIAGENT.md),
[the issue loop](ONE_TASK_LOOP.md), [status in the shell](REPL.md), and
[learning](LEARNING.md).
