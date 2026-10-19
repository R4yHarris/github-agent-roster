Follow root [AGENTS.md](../AGENTS.md). Implement only the current prompt in
[prompts/](../prompts/). Ground all work in the product spec,
[docs/FEATURE_SPEC.md](../docs/FEATURE_SPEC.md): tie each change to a section 5
feature, honor sections 3 and 7, cite the section in the PR body, and do not
contradict the spec. Do not add a Kanban DB. Zero runtime deps unless a prompt
says otherwise. Ray approved the full incremental Rust migration on 2026-10-15;
follow [the migration prompt](../prompts/04-rust-migration.md). This branch is
Rust-only: remove the Node implementation and oracle-retention scaffolding.
Use main/Git history as the behavioral reference, not a parallel Node runtime
or frozen checkout. Port acceptance checks into native tests; removal is not
proof of completeness. Read the scoped
[Rust](instructions/rust.instructions.md) and
[Cargo](instructions/cargo.instructions.md) instructions for Rust work.

Require the latest reviewed Git submodule at
[`vendor/github-agent-contracts`](../vendor/github-agent-contracts). Clone with
`git clone --recurse-submodules` or run `git submodule update --init --recursive`.
Do not copy contracts source into this tree or rewrite files in the submodule.
Use [`resolve_contracts_path`](../rust/paths.rs) to check the submodule, then
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
