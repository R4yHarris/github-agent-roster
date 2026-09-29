# Roster principals

The **GitHub App is the principal, not the LLM**. GitHub authenticates the App
through its installation token. The contracts policy constrains operations;
worker names, model choices, and `AI_*` run metadata do not grant authority.

## Seats and contracts roles

The roster seat `coder` maps to the contracts role `coder`. A recipe's
`principal: coder` names that role; it does not select an App key or create
permissions. A trusted launcher binds the worker to the operator-selected coder
App. The worker cannot choose a different App ID, key path, or role.

The required coder-only worker baseline is:

| Roster seat | Contracts role | Assignment | Policy allow-list |
| --- | --- | --- | --- |
| `coder` | `coder` | One coder worker | `commit_branch`, `open_pr`, `comment`, `label` |
| `merger` | `merger` | Empty; no recipe seat | `merge` in reviewed policy |
| `deploy` | `deploy` | Empty; no worker or credentials | `[]` |

Merger and deploy are reserved here, not supported v0 recipe seats. An optional
planner also uses `principal: coder`; planning grants no merge or deploy
authority. See [seats and recipes](SEATS.md). An explicit publish request
uses `--merge-when-green` only through the SDK, which checks the separately
reviewed `merger.merge` grant and required GitHub checks. The model cannot
invoke publication through a coder tool.

The checked-in, human-owned [policy](../agent-policy.yml) currently grants
`merger` the `merge` capability. An empty roster assignment is not a policy
denial. A deployment that must forbid App merging needs a human to remove
that grant and stop using `--merge-when-green`; workers must not edit policy.

## Publication boundary

Publish only through the pinned `v0.2.0`
[vendor publisher](../vendor/github-agent-contracts/scripts/agent-pr.mjs), from
the current worktree's repository root:

```sh
node vendor/github-agent-contracts/scripts/agent-pr.mjs --message "..." --merge-when-green
```

The operator provisions `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` outside
git. Review and stage only intended changes and pass applicable checks before
publishing a feature branch. Missing App configuration or denied policy means
stop with changes uncommitted: no direct `git commit`, `git push`, `gh pr create`,
or signed-in human fallback. Do not copy, rewrite, or replace the vendor helper.
See [dependency setup](DEPENDENCY.md) and [key custody](THREAT_MODEL.md).

## Later WSL worker

- Give the approved publisher only the configured App credentials; it checks
  the reviewed merger grant for explicit merge requests. Keep deploy credentials
  absent; the helper does not switch keys based on role names.
- No human `gh` login, human PAT, human Git credential helper, or forwarded human
  SSH identity may be available in the WSL worker. Do not share Windows `gh`
  authentication or inject human tokens as environment fallbacks.
- Keep the operator's Windows authentication separate and unchanged. Human
  review and `AI-Eval` happen outside the worker; the SDK alone may merge
  after policy, required checks, and repository protections allow it.
- The [current one-task loop](ONE_TASK_LOOP.md) expects authenticated `gh` for
  issue reads. A trusted executor must arrange App-authenticated reads before
  using that loop in WSL; the CLI does not provision this credential isolation.

These are deployment requirements, not machine configuration performed by this
document. Follow the contracts
[orchestration-machine guide](../vendor/github-agent-contracts/docs/ORCHESTRATION-MACHINE.md).
