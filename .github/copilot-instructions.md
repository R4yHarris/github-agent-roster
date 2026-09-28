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

When App env is set, publish from the repository root with
`node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "..."`.
Never `git commit` as the signed-in human when App env is set.
