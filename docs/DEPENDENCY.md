# Contracts dependency

[github-agent-contracts](https://github.com/R4yHarris/github-agent-contracts)
provides identity, policy, trailers, and the publishing script. This repository
requires it as the Git submodule at
[`vendor/github-agent-contracts`](../vendor/github-agent-contracts), pinned to
tag `v0.2.0`. Keep the submodule pointer; do not copy contracts source into this
repository or rewrite files inside the submodule.

## Checkout

Clone with submodules:

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
```

For an existing checkout or a new worktree, initialize the pinned dependency
from that checkout's root:

```sh
git submodule update --init --recursive
```

## Resolution

[`resolveContractsPath`](../src/lib/paths.mjs) returns the first usable contracts
directory in this order:

1. `vendor/github-agent-contracts` inside the roster checkout.
2. `GITHUB_AGENT_CONTRACTS`, when set. Relative values resolve against the
   caller's current working directory; absolute paths are normalized.
3. `../github-agent-contracts`, the sibling clone beside the roster checkout.

The submodule and sibling locations are relative to the roster installation,
not the caller's current working directory. A candidate is usable only when
`scripts/agent-pr.mjs` is a file. Missing or incomplete candidates are skipped;
if none contains that file, resolution fails with the searched locations and
the submodule initialization command.

`GITHUB_AGENT_CONTRACTS` is optional, but if set it must be a non-empty string.
Blank or non-string values are errors, even when the submodule is initialized.
Unset the variable rather than setting it to an empty value.

## Publish

With `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` set, publish from the
current feature worktree's repository root using the initialized submodule:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "..."
```

The publisher also requires a feature branch and a human-owned root
`agent-policy.yml` granting the coder publishing capabilities, matching the
reviewed policy on origin's default branch. Do not create or edit policy as
part of dependency setup.

A new issue worktree may need `git submodule update --init --recursive` run
from its own root before manual publication. The builtin `--publish` path
initializes it, selects reviewed task files, and invokes the SDK from that
root; without the flag, it prints the next command but does not invoke it.

Do not commit as the signed-in human when App environment variables are set.
Never commit tokens, private keys, or `.env`.

Run the Node 20+ ESM tests with `node --test`. No runtime packages are required.
