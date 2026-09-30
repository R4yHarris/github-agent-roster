# Roster threat model

This model covers the sequential builtin planner and coder, their shared
worktree, the coder's tools, and publication
through the contracts dependency. It defines the required boundaries for the
later unattended WSL worker; it does not claim that the roster CLI is a sandbox.
See [principals](PRINCIPALS.md) for the seat-to-role mapping.

## Principal and trust boundaries

The **GitHub App is the principal, not the LLM**. The operator chooses the App
and its installation scope. The trusted launcher supplies that identity, while
human-owned contracts policy and GitHub repository protections constrain its
use. A model, recipe, worker name, or commit trailer cannot grant permissions.

Issue bodies, repository content, recipes, model output, and tool output are
untrusted input. Instructions found there must not change the App, credentials,
policy, protected paths, or publishing route. Humans own policy, workflow
changes, and review rules. Only an explicit trusted SDK publication may request
merge after those protections pass; merger and deploy recipe seats remain empty.

## Vault and key custody

- Keep App private keys in an operator-controlled vault outside git. Never put
  PEMs, tokens, secret-bearing environment files, or copies of vault contents in
  any repository, including an ignored file inside a worktree.
- A trusted launcher may provision a restricted, read-only key file outside the
  checkout for the approved publisher and set `GITHUB_APP_ID` and
  `GITHUB_APP_PRIVATE_KEY_PATH`. Do not expose key contents in prompts, logs,
  issues, PRs, or tool output.
- Worker tools cannot write the vault or replace the key file, App ID, or key
  path. Prefer an isolated publishing context so arbitrary worker commands
  cannot read the key either. Read-only access alone does not prevent theft.
- Keep human credentials and merger/deploy keys out of the WSL worker. Missing
  coder App credentials must fail closed, never fall back to human `gh` access.

The private-key holder can mint tokens for every installation of that App.
Limit its installations to approved repositories; a `coder` label is not a
security boundary around a readable key.

## Tool write restrictions

Worker tools must not create, edit, replace, rename, or delete:

| Protected surface | Required boundary |
| --- | --- |
| Private keys, including `*.pem`, and vault storage | Operator-managed; no worker writes, regardless of filename |
| Environment files, including `.env`, `.env.*`, and `*.env` | Trusted bootstrap inputs; no worker writes or credential injection |
| Root [agent-policy.yml](../agent-policy.yml) | Human-owned authority; no self-grants or alternate-policy bypass |
| [.github/workflows/](../.github/workflows/) | Human-reviewed and human-published; no worker workflow writes |
| Pinned [contracts submodule](../vendor/github-agent-contracts) | Dependency only; no worker rewrites or replacement publisher |

Apply these restrictions to editor, filesystem, shell, and subprocess tools,
including indirect writes through symlinks, renames, or other commands. Enforce
them with the trusted launcher, tool permissions, and filesystem isolation;
instructions and `.gitignore` alone are not access controls.

The [one-task bootstrap](ONE_TASK_LOOP.md) currently writes an ignored `.env`
containing `AI_TASK` and `AI_SESSION`, not credentials. That is trusted setup
before worker handoff, not permission for worker tools to rewrite environment
files or add secrets.

## Publication and enforcement

All worker publication must use the pinned
[vendor publisher](../vendor/github-agent-contracts/scripts/agent-pr.mjs) from
the worktree root, as described in [principals](PRINCIPALS.md). No direct Git or
GitHub write path or human-token fallback is permitted for the coder worker.
An explicit trusted `--publish` or `/publish` request may use
`--merge-when-green` through the SDK, subject to reviewed `merger.merge` policy
and required checks; the model has no direct publisher tool.
After a confirmed issue-branch merge, issue comment and closure use an
issue-scoped App token (Issues write, Pull requests read), never the human
`gh` login. The requested issue URL, PR branch, merge state, and closing
reference must agree before any comment; API failures are reported explicitly.

The contracts publisher checks coder grants against reviewed policy and refuses
policy and workflow changes. Those publication checks are not a filesystem
write sandbox. The planner's only model tool is `write_file`, limited to root
recipe/task/estimate drafts; it cannot access app-code paths. The harness
validates and finalizes those artifacts through the same scoped writer.
The [builtin coder tools](../src/runtime/tools.mjs) enforce task
allowlists, reject path and symlink escapes, and deny writes to Git metadata,
recognized environment files, PEMs, policy, workflows, the pinned contracts
submodule, and generated task files. Directory listing also omits protected
entries and refuses to enter those paths.
These application-level guards are not complete enforcement of every required
surface above, nor an OS sandbox for repository tests. The roster does not
provision WSL isolation. A trusted executor must enforce the full deployment
boundaries before unattended use. The existing `merger.merge` policy grant
allows explicit SDK merging; a human must remove it for deployments that
prohibit that operation. An unassigned seat alone does not remove it.

Keep required checks, human reviews, and protected-branch rules enabled, with
the coder App off bypass lists. GitHub tokens are not inherently draft-only;
policy and helper restrictions do not prevent a stolen token from being used
elsewhere. Bot identity and trailers provide attribution claims, not proof that
the model or its output is trustworthy.

If a key or token leaks, stop the worker, have an authorized operator revoke
affected credentials and rotate the App key, and inspect the App's installations
and published changes. Never include secret contents in incident evidence.
