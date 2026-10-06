# Roster principals

The **GitHub App is the principal, not the LLM**. GitHub authenticates the App
through its installation token. The contracts policy constrains operations;
worker names, model choices, and `AI_*` run metadata do not grant authority.

## Harness operating principles

These apply to every seat and to the harness that sequences them
([feature spec](FEATURE_SPEC.md) §3, §5.2, §5.4, §5.6).

- **Work to completion.** A run exists to deliver the Ask through plan, code,
  verification, and review, not to stop at the first obstacle. Recoverable
  failures are steered toward a result; only unrecoverable ones end the run.
- **Do not repeat the same thing and expect a different result.** A failed
  attempt is evidence. Retrying the same action against the same unchanged
  state, model, or endpoint is not progress; change the approach first.
- **Get a different perspective.** When an auto-routed fleet profile fails as a
  route (it serves a substituted model, times out, or stalls), the harness
  quarantines that profile for the run and continues the *same seat* on the
  next eligible profile. The planner, coder, and reviewer each recover this
  way. A coder retry keeps the previous attempt's worktree edits and diff
  baseline and is told to continue from them with a different approach.
- **Preserve progress.** Recovery keeps RECIPE.yml, TASK.md, ESTIMATE.md, and
  worktree edits; it archives only the per-attempt CONTEXT.md and RESULT.md.
- **Never switch models silently.** Every reroute is logged
  (`Route recovery: seat=… profile=…`), recorded in `routeAttempts`, and
  measured under the model that actually answered. A substituted response is
  never accepted under the locked model's identity.
- **Fail honestly when no alternative remains.** If no eligible profile is
  left, the run stops with the original error and `Route recovery exhausted`.
  Task failures such as a check failing twice (§5.4) still stop for a human;
  rerouting is for endpoint failures, not for weakening checks. Saved,
  non-routed models are never rerouted.

## Builtin coder conduct

A seat is a bounded principal, not a chat or the human GitHub user. Its
[conduct file](../principals/coder.md) requires work products: TASK.md, a scoped
diff, test evidence, and an honest RESULT.md, not status theater.

[`loadPrincipal`](../src/seats/principal.mjs) reads the roster installation's
`principals/coder.md` before execution. Missing, empty, oversized, and
symlinked files fail explicitly. Markdown is conduct, not a policy parser:
text claiming merge permission cannot change the fixed `coder` role,
`commit_branch`/`open_pr` capabilities, or frozen deny rules.

The returned `deny.read` and `deny.write` predicates are the same rules used
by the [runtime tools](../src/runtime/tools.mjs). Secrets (`.env`, `.env.*`,
`*.pem`, and `.roster/vault`), Git metadata, worktree escapes, and symlinks
cannot be accessed. Policy, workflows, the contracts submodule, and
harness-owned task/result files cannot be edited. Writes must also match
TASK.md's allowed paths. The loop offers only configured builtin tools;
principal prose cannot add a tool, increase the turn budget, or grant merge,
protected-branch push, or deploy.

The coder stops at a verified result or its turn budget and states what
failed or remains unverified. With no LLM endpoint the deterministic stub
remains non-editing and reports that implementation and tests were not run.
Publication remains a separate reviewed harness action; a parent-requested
`--merge-when-green` does not grant merger authority to the coder.

## Builtin reviewer conduct

The [reviewer conduct file](../principals/reviewer.md) instructs a separate
sequential seat to inspect acceptance checks, RESULT.md, and the diff.
Its frozen local capability is comment-only. It receives no model file tools,
cannot write `src/` or any other source, and cannot merge or publish.
Only the harness writes [REVIEW.md](REVIEW.md). Neither this conduct file
nor a `principal: reviewer` recipe entry grants GitHub App policy capability.
Malformed tool calls are refused and result in a failing report.

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
authority. The reviewer has no separate contracts App role; its comments
are local review evidence, not a GitHub approval. See [seats and recipes](SEATS.md).
An explicit publish request
uses `--merge-when-green` only through the SDK, which checks the separately
reviewed `merger.merge` grant and required GitHub checks. The model cannot
invoke publication through a coder tool.

The checked-in, human-owned [policy](../agent-policy.yml) currently grants
`merger` the `merge` capability. An empty roster assignment is not a policy
denial. A deployment that must forbid App merging needs a human to remove
that grant and stop using `--merge-when-green`; workers must not edit policy.

## Publication boundary

Publish only through the pinned fail-closed `v0.2.1`
[vendor publisher](../vendor/github-agent-contracts/scripts/agent-pr.mjs), from
the current worktree's repository root:

```powershell
node vendor\github-agent-contracts\scripts\agent-pr.mjs --message "<subject plus Model, Summary, and how-to-test sections>" --model GPT-6.1-Sol --merge-when-green
```

The operator provisions `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` outside
git. Review and stage only intended changes and pass applicable checks before
publishing a feature branch. Missing App configuration or denied policy means
stop with changes uncommitted: no direct `git commit`, `git push`, `gh pr create`,
or signed-in human fallback. Do not copy, rewrite, or replace the vendor helper.
Post-merge issue commenting uses a separate, narrowly scoped App token with
Issues write and Pull requests read permissions. Roster checks the reviewed
coder comment and merger merge grants, verifies the merged issue-branch PR
and open issue, and never uses human GitHub credentials for that comment.
The human closes the issue only after AI-Eval.
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
