Follow root [AGENTS.md](../AGENTS.md). Implement only the current prompt in
[prompts/](../prompts/). Do not add a Kanban DB. Zero runtime deps unless a prompt
says otherwise. Node 20 ESM.

Require the `v0.2.0` Git submodule at
[`vendor/github-agent-contracts`](../vendor/github-agent-contracts). Clone with
`git clone --recurse-submodules` or run `git submodule update --init --recursive`.
Do not copy contracts source into this tree or rewrite files in the submodule.
Use [`resolveContractsPath`](../src/lib/paths.mjs) to check the submodule, then
`GITHUB_AGENT_CONTRACTS`, then `../github-agent-contracts`. Fail if no candidate
contains the `scripts/agent-pr.mjs` file. See
[the dependency guide](../docs/DEPENDENCY.md).

Copilot subagent names `planner` and `coder` map to Roster's sequential builtin
seats in one run/worktree, not separate chats, a second queue, Hermes Kanban,
or a new runtime. See the [GHCP bridge](../docs/GHCP.md).
The GitHub App is the publishing principal, not the model or subagent name;
policy and workflows are human-owned.

When `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` are set, publish reviewed
GHCP changes only from the feature worktree's repository root using
`node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "<conventional subject>" --merge-when-green`.
Never leave a draft PR for the human. When `GITHUB_APP_ID` is set, never
`git commit` as the signed-in human, even if the private-key path is missing;
stop and report incomplete App credentials instead. Never use `gh pr create`
or `git push` with human credentials. If `--merge-when-green` fails with HTTP
422, stop and report that Checks permission is not accepted on the installation;
do not use a workaround. Do not edit vendor sources, the human-owned
`agent-policy.yml`, or `.github/workflows/*`.
