# GHCP identity bridge

Copilot subagent names `planner` and `coder` map to Roster's
[sequential builtin seats](MULTIAGENT.md): one run executes planner then coder
in the same issue worktree. They do not create separate chats, another queue,
Hermes Kanban, or a new runtime. GitHub Issues and PRs remain the board; see
the [SDLC](SDLC.md).

## Publish from the feature worktree

For an explicitly requested, reviewed GHCP change, when `GITHUB_APP_ID` and
`GITHUB_APP_PRIVATE_KEY_PATH` are set, run from the **current feature worktree's
repository root**. Initialize the pinned `v0.2.0` contracts submodule there if
needed, following the [dependency guide](DEPENDENCY.md):

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "<conventional subject>" --merge-when-green
```

The same flag applies to explicit builtin publication. It does not add a
merger or deploy seat or bypass human-owned policy, reviews, or required checks.
Keep the [hard boundaries](../AGENTS.md): do not
edit the contracts submodule, [agent-policy.yml](../agent-policy.yml), or
[workflows](../.github/workflows/).

Never `git commit` as the signed-in human when App environment is set, and
never use `gh pr create` or `git push` with human credentials to publish. If
`--merge-when-green` fails with HTTP 422, stop and report that Checks
permission is not accepted on the installation. Do not use a workaround or
fall back to human credentials.
