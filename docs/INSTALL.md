# Install and bootstrap

Roster uses Node 20+ ESM, Git, and `gh`; it has no runtime npm dependencies.
For the complete task loop and App publication, install from a persistent
source checkout with its pinned fail-closed `v0.2.1` contracts submodule:

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
npm install -g .
roster --help
```

Run `roster` in an interactive terminal to open the human shell; `/quit` or
Ctrl+C exits. Run `node --test` in the clone to check the source.

For an existing checkout or new worktree, run
`git submodule update --init --recursive` from its root. The publisher is
resolved by [`resolveContractsPath`](../src/lib/paths.mjs): initialized
submodule first, then `GITHUB_AGENT_CONTRACTS`, then the sibling clone.
None of these paths authorizes copying or changing contracts sources.
See [the dependency guide](DEPENDENCY.md).

## Local npm executable

The package declares `roster` as its bin and requires Node >=20. A local
checkout can exercise the packaged CLI without installing dependencies:

```sh
npm exec --yes --package . -- roster --help
```

The npm `files` list includes the CLI, templates, skills, docs, tracked
config example, and pinned contracts scripts. It excludes roster and
vendor test directories. `npm pack --dry-run --json` shows the actual file
list without producing a tarball. If a human publishes a package to a
registry later, `npx` can use the same bin; this repository does not
assume that version `0.0.0` is published.

`roster init` works in the **current directory**, including when invoked
through the packaged bin. It exclusively copies
`roster.config.example.yml` and `ROSTER-POLICY-NOTE.md`, reporting existing
files as kept. It does **not** create or overwrite `agent-policy.yml`, a
workflow, credentials, or a private config. The note is for human policy
review, not a policy file. A transient `npx` installation does not provide
a stable location for private runtime settings; use a persistent roster
checkout for the configured coder loop.

In a persistent roster checkout, copy the reviewed example into its
ignored private config before changing model settings:

```powershell
New-Item -ItemType Directory -Force .roster
if (-not (Test-Path .\.roster\config.yml)) {
  Copy-Item .\roster.config.example.yml .\.roster\config.yml
}
```

Do not put API key values, App PEMs, or tokens in either config. Use
[endpoint profiles](ENDPOINTS.md) and the [file vault](LLM.md) for LLM
keys; keep the App key outside Git.

## Preflight and publication

From the target repository root, `roster doctor` checks Node 20, a usable
contracts publisher, presence of both App environment variables, a
regular root `agent-policy.yml`, and the trailer workflow. It makes no
network call and never prints App values or private-key paths. A failed
check exits nonzero; it does not repair human-owned policy or workflows.

After tests and review, publish only from the current feature worktree's
repository root with the GitHub App:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "<subject plus Model, Summary, and how-to-test sections>" --model "$AI_MODEL" --merge-when-green
```

Set the real model first; GHCP sessions use `AI_MODEL=GPT-6-Sol`.
See the complete [GHCP publication example](GHCP.md). Missing or unknown
models cannot publish. Never commit or push as the signed-in human when App credentials are set.
The SDK enforces reviewed policy and required checks; do not edit
`agent-policy.yml` or `.github/workflows/*` to bypass a denial. See
[trailer CI](CI.md), the [interactive shell](REPL.md), and the
[GitHub board](BOARD.md).
