# GHCP identity bridge

Copilot subagent names `planner` and `coder` map to Roster's
[sequential builtin seats](MULTIAGENT.md): one run executes planner then coder
in the same issue worktree. They do not create separate chats, another queue,
Hermes Kanban, or a new runtime. GitHub Issues and PRs remain the board; see
the [SDLC](SDLC.md).
The authenticated GitHub App is the publishing principal, not the model or
subagent name. Only human-owned policy can grant capabilities.

## Parent Copilot handoff

The parent Copilot coordinates the request by calling Roster, rather than
starting separate coding chats or writing a second queue:

```sh
node src/cli.mjs run --issue N --runtime builtin --seats planner,coder
```

In a human TTY, the equivalent is `/run N` in the [Roster shell](REPL.md).
The parent reviews the resulting worktree and tests, then invokes the
contracts publisher below for an explicitly requested publication. Copilot
subagent names label Roster seats; they are not GitHub identities or policy
grants.

## Publish from the feature worktree

For an explicitly requested, reviewed GHCP change, when `GITHUB_APP_ID` and
`GITHUB_APP_PRIVATE_KEY_PATH` are set, publish only through the contracts
script from the **current feature worktree's repository root**. Initialize the
pinned `v0.2.0` submodule there if needed, following the
[dependency guide](DEPENDENCY.md):

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "<conventional subject>" --merge-when-green
```

The same flag applies to explicit builtin publication. It does not add a
merger or deploy seat or bypass human-owned policy, reviews, or required checks.
Keep the [hard boundaries](../AGENTS.md): do not edit the contracts submodule,
human-owned [agent-policy.yml](../agent-policy.yml), or human-owned
[workflows](../.github/workflows/).

Never `git commit` as R4yHarris (the signed-in human) when `GITHUB_APP_ID`
is set, even if the private-key path is missing. Stop and report incomplete
App credentials rather than falling back to the human. Never use `gh pr create`
or `git push` with human credentials to publish. If
`--merge-when-green` fails with HTTP 422, stop and report that Checks
permission is not accepted on the installation. Do not use a workaround or
fall back to human credentials.
