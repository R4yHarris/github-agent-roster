# Assignment

- Issue URL: <current repository's issue URL>
- Issue number: 42
- Title: <issue title>

## Ask

<Copy the issue body verbatim, including its scope and constraints.>

## Coder handoff

If the one-task loop already generated an assignment, preserve its metadata and
unmodified Ask. A human can append these handoff sections; builtin tools cannot
rewrite generated assignments. Replace placeholders and the example number `42`.

- Assigned worktree: <existing absolute worktree path>
- Branch: <assigned feature branch>
- Task: [TASK.md](TASK.md)
- Recipe: [RECIPE.yml](RECIPE.yml)
- Seat / principal: `coder` / `coder`
- Worker: `builtin` (must match the recipe)
- Skills: `implement-task`, then `run-tests`
- `AI_TASK`: `issue-42`
- `AI_SESSION`: <orchestrator-provided session identifier>

Load the supplied `AI_*` values into the worker environment. The one-task loop
writes task and session values to an ignored `.env`; never commit that file or
put App credentials in these handoff documents.

## Boundaries

Stay in the assigned worktree and follow its repository instructions and the
task's allowlist. There is one active coder seat, not a worker swarm. Do not
create extra issues, launch additional workers, edit human-owned policy, merge,
or deploy. A recipe describes work; it does not grant GitHub permissions.

## Verification and return

Use `node --test` for automated checks, through `run_test` in the builtin loop.
Record acceptance evidence with actual commands, test names, results, and any
failures or blockers. Return a summary of the bounded changes, verification,
and remaining risks. Do not claim success for unrun or skipped checks.

## Publication

With `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` set and the required
`v0.2.0` contracts submodule initialized, publish from this worktree's repository
root through the contracts publisher:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "<type>: issue 42"
```

Use a message describing the actual change. Human-owned policy must authorize
publication. Missing credentials, dependency, or permission are blockers, not a
reason to copy contracts code, edit policy, or publish as the signed-in human.
If App env is absent, return the local handoff without claiming publication.
The human reviews the PR, controls merge, and posts `AI-Eval:`.
