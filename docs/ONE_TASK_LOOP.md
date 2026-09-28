# One-task loop

`runIssue(issueNumber)` in `src/lib/issue.mjs` implements
`roster run --issue N`. It can also be called directly:

```js
import { runIssue } from './lib/issue.mjs';

await runIssue('42');
```

The caller must run it from within the repository containing the GitHub issue.
The live command requires Git, an installed and authenticated GitHub CLI (`gh`),
and an accessible GitHub issue on the **current repository's origin remote**.
`runIssue` finds the repository root with `git rev-parse --show-toplevel`, reads
its GitHub HTTPS or SSH origin, and runs
`gh issue view N --repo OWNER/REPO --json number,title,body,url` there. It does
not read cached issue data or create issues. A missing issue, empty issue body
(the Ask), or failed command is an error; no worktree is created if issue
lookup fails.

For issue `N`, the function creates branch `issue-N` from the current HEAD in
`.worktrees/issue-N`, writes `ASSIGNMENT.md` with the issue URL, number, title,
and unmodified body under **Ask**, and writes an ignored `.env` file:

```dotenv
AI_TASK=issue-N
AI_SESSION=roster-<UTC timestamp>
```

It returns the issue, worktree and file paths, task, session, and `nextCommand`.
It prints the worktree location and, for the worker **after editing inside the
worktree and loading `.env` into its environment**, exactly:

```sh
node $GITHUB_AGENT_CONTRACTS/scripts/agent-pr.mjs --message "feat: issue N"
```

Set `GITHUB_AGENT_CONTRACTS` to the sibling contracts clone path in the worker
environment. The printed command uses POSIX shell variable syntax; PowerShell
users must use their shell's environment-variable syntax when executing it.
This module only prepares one coder seat and prints the publishing command: it
does not start a worker, merge, or open additional issues. Merge remains
human-controlled.

Tests inject `runCommand`, `fileSystem`, `now`, and `log` into `runIssue` to
simulate git, gh, file writes, timestamps, and output without a network call.
Run the focused suite with `node --test tests/issue.test.mjs`.
