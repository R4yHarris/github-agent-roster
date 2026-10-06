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
  never accepted under the locked model's identity. A substitution is also
  remembered for 24 hours in the ignored `.roster/runs/route-quarantine.json`.
  Later auto-routed runs then start on an honest profile and log
  `Route quarantine: skipping profile=…`. If every eligible profile is
  quarantined, routing falls back to the unfiltered choice.
- **Fail honestly when no alternative remains.** If no eligible profile is
  left, the run stops with the original error and `Route recovery exhausted`.
  Saved, non-routed models are never rerouted.
- **Work to completion; escalate perspective, not effort on the same context.**
  When a coder context exhausts its repair or turn budget, or repeats a denied
  action, that context is stuck, but the task is not proven impossible. The
  harness starts a fresh coder context (`Perspective escalation N of 2`). It uses
  the next eligible fleet profile when auto-routing, and the same model
  otherwise. Worktree edits are kept, and the context gets the last failure
  plus an instruction to decide which side (test or implementation) is wrong
  and stop alternating between them. Escalation never weakens checks and
  never applies to security denials. After two escalations, the run stops for
  a human (§5.4).
- **Scope steers; security denies.** TASK.md Allowed Files are the planned
  scope. Security boundaries always stop the run: secrets, `.git`, policy,
  workflows, `vendor/`, harness-managed files, `.roster/`, symlinks, and
  worktree escapes. Other repository files outside the plan may be written up to
  `seat.scope_expansion` files (default 3; `0` restores strict scope). Each
  expansion is logged, listed in RESULT.md and in the PR body under "Files
  outside planned scope", and judged by the reviewer, who fails it unless it is
  necessary and minimal. Hitting the cap re-scopes rather than fails: the
  blocked files are evidence, so the orchestrator raises the budget (at most
  twice, never above 16) and continues the coder in a fresh context (§5.3).
- **A failed review is feedback; iterate, then switch perspective.** A completed
  reviewer fail sends its per-check findings back to a fresh coder context
  (`Review repair N of 2`). A repair that leaves the same checks unmet is the
  same move twice, so the next round must change strategy and, when
  auto-routing, uses a different fleet profile (§5.5).

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
