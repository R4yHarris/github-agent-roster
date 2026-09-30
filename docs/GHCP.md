# GHCP identity bridge

Copilot subagent names `planner` and `coder` map to Roster's
[sequential builtin seats](MULTIAGENT.md): one run executes planner, coder,
then a read-only builtin reviewer
in the same issue worktree. They do not create separate chats, another queue,
Hermes Kanban, or a new runtime. GitHub Issues and PRs remain the board; see
the [SDLC](SDLC.md).
The authenticated GitHub App is the publishing principal, not the model or
subagent name. Only human-owned policy can grant capabilities.

## Parent Copilot handoff

The parent Copilot coordinates the request by calling Roster, rather than
starting separate coding chats or writing a second queue:

```sh
node src/cli.mjs run --issue N --runtime builtin --seats planner,coder,reviewer
```

In a human TTY, the equivalent is `/run N` in the [Roster shell](REPL.md).
The parent reviews the resulting worktree and tests, then invokes the
contracts publisher below for an explicitly requested publication. Copilot
subagent names label Roster seats; they are not GitHub identities or policy
grants.
Roster-managed `--publish` and REPL `/publish` require a passing
[REVIEW.md](REVIEW.md) unless `--skip-review` is explicit. A direct call to
the contracts publisher cannot enforce that in-process gate: review the
code and result yourself before using the manual handoff.

## Publish from the feature worktree

For an explicitly requested, reviewed GHCP change, when `GITHUB_APP_ID` and
`GITHUB_APP_PRIVATE_KEY_PATH` are set, publish only through the contracts
script from the **current feature worktree's repository root**. Initialize the
pinned fail-closed `v0.2.1` submodule there if needed, following the
[dependency guide](DEPENDENCY.md):

GHCP sessions set the real model explicitly before publication. In PowerShell:

```powershell
$env:AI_PROVIDER = "github-copilot"
$env:AI_MODEL = "GPT-6.1-Sol"
$env:AI_MODEL_VERSION = "-"
$env:AI_EFFORT = "x"
$env:AI_CONTEXT_MAX = "1000000"
$env:AI_CONTEXT_USED = ""
$env:AI_CONTEXT_OUT = ""
$env:AI_SESSION = "ghcp-$(Get-Date -Format yyyyMMdd)-$PID"
$env:AI_TASK = "feature-branch-slug"
$message = @'
fix: refuse publish without a model id

## Model

GPT-6.1-Sol

## Summary

Require a real model on every publish path and retain reviewed change and test instructions.
GHCP used/out are `-` (unknown); 1M is declared capacity, not used tokens.

### How to test

Run `node --test` from the feature worktree root.
'@
node vendor\github-agent-contracts\scripts\agent-pr.mjs --message $message --model GPT-6.1-Sol --merge-when-green
```

Use a summary of the actual reviewed changes, not just the conventional
subject. The SDK derives the PR body from the message, so direct calls must
include `## Model`, `## Summary`, and how to test. Roster's builtin and REPL
paths add these sections automatically. Without a completed seat, they require
`AI_MODEL` or explicit `/publish --model MODEL`, set `github-copilot`, and
leave used/out as `-`. They do not borrow the configured vLLM/cloud model.
A completed seat's actual response-backed metadata wins over those declarations
and is passed with `--model`; missing or invalid IDs fail with `set model`
before the SDK. The pinned publisher
also exits nonzero for missing or unknown model input.
When ready to PR from this Copilot session, the canonical command is
`node vendor\github-agent-contracts\scripts\agent-pr.mjs --message "..." --model GPT-6.1-Sol --merge-when-green`.
Other configured workers retain their actual served model ID.

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
