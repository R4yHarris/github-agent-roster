# Contracts dependency

[github-agent-contracts](https://github.com/R4yHarris/github-agent-contracts)
provides identity, policy, trailers, and the publishing script. This repository
requires it as the Git submodule at
[`vendor/github-agent-contracts`](../vendor/github-agent-contracts), pinned to
tag `v0.2.1`, which refuses missing or unknown model IDs. Keep the submodule
pointer; do not copy contracts source into this
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

See [installation and bootstrap](INSTALL.md) for the local npm bin,
private configuration, and preflight checks.

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
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "<subject plus Model, Summary, and how-to-test sections>" --model "$AI_MODEL" --merge-when-green
```

The publisher also requires a feature branch and a human-owned root
`agent-policy.yml` granting the coder publishing capabilities and, for
`--merge-when-green`, the merger capability, matching the reviewed policy on
origin's default branch. Do not create or edit policy as part of dependency
setup.

A new issue worktree may need `git submodule update --init --recursive` run
from its own root before manual publication. The builtin `--publish` path
initializes it, selects reviewed task files, and invokes the SDK from that
root; without the flag, it prints the next command but does not invoke it.
Every Roster publish path resolves the model from `config.llm.model`, then
`AI_MODEL`, then `ROSTER_MODEL`, and passes it explicitly with `--model`.
An absent or invalid ID aborts with `set model` before invoking the SDK.
Completed runs retain the actual coder model even if configuration changes.
Model-free preparation and stub runs report that publication is unavailable
rather than printing a runnable model-free command.

Roster-generated messages include `## Model`, `## Summary`, and how to test,
plus an issue closing reference when applicable. For direct SDK publication,
include those sections in `--message`; do not send a subject-only PR body.
GHCP sessions set `AI_MODEL=GPT-6-Sol` and `AI_PROVIDER=github-copilot`;
see the complete [GHCP example](GHCP.md).

Do not commit as the signed-in human when App environment variables are set.
Never commit tokens, private keys, or `.env`.

## Trailer CI

The [composite action](../.github/actions/check-agent-trailers/action.yml)
passes the PR base and head SHAs to the pinned submodule's
`scripts/check-pr-agent-trailers.mjs`, rather than the roster's local copy.
The human-owned [workflow](../.github/workflows/check-agent-trailers.yml) must
check out full history with `persist-credentials: false` and
`submodules: recursive`, then use Node 20 before running that action. Do not
check out the base revision before the local action: that replaces the PR
checkout and can leave its checker without required files. The workflow also
needs a separate `tests` job running `node --test tests/*.test.mjs` on Node 20.

Run the Node 20+ ESM tests with `npm test`. No runtime packages are required.
