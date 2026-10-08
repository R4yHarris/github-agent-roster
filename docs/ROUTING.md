# Opt-in fleet routing

Onboarding selects one default endpoint/model and seeds a private
[fleet catalog](FLEET.md). Normal `roster run --issue N` keeps that
configuration; neither fleet discovery nor a read-only recommendation
silently rewrites it. GitHub Issues and PRs remain the board, not a
routing database or concurrent worker runtime.

## Selecting a profile

[`route.mjs`](../src/lib/route.mjs) selects only models actually registered
in `.roster/fleet.yml`:

1. Prefer the highest acceptance rate among fleet models with at least
   **three distinct human evaluations** for the requested task class and
   effort, sufficient median human-rated difficulty, and a positive
   declared context limit. Ties prefer more samples, then the shorter live
   [admission queue](FLEET.md#concurrency-admission), then stable profile
   and effort ordering. A defect still turns a later human acceptance into
   a derived reject; automatic test/excellence passes are not human evals.
2. Otherwise use matching [capability priors](CAPABILITIES.md) and fleet
   task-class hints. Profile-specific priors take precedence over model
   priors. A prior's suggested difficulty must cover the request; a bare
   class hint has no measured difficulty claim. Prefer explicit fleet
   class hints, then smaller sufficient hardware and context, then the
   shorter live admission queue, then higher declared concurrency as a
   **weak tie-break**, then stable ID ordering. Queue depth is congestion,
   not failure. No throughput or benchmark claim is made.

Both tiers exclude a model when its derived **seat + model** difficulty
ceiling is below the task difficulty, even if a capability prior is stronger.
`chooseRoute` accepts a `seat` (default `coder`) and uses only that seat's
human evaluations in the evidence tier. A model with no evaluations for that
seat keeps its existing prior behavior; automatic-only runs do not establish
a ceiling. The [learning ledger](LEARNING.md)
starts at 2, raises only after three consecutive clean at-level accepts, and
lowers on reject (FEATURE_SPEC 5.6). Median difficulty and the three-sample
threshold still apply; a ceiling does not replace them.

A route caller can supply a minimum token context requirement. A declared
limit must meet that exact threshold. Unknown `context_max: 0` cannot
satisfy a positive requirement or qualify the human-evidence tier.
The seeded default may remain a first-run prior choice when no minimum is
specified, but its context stays **unknown**; the prior does not fabricate
capacity or AI-Run usage. A requested missing profile ID is an error.
Models present only in evaluations or fictional examples are never chosen.

## Read-only recommendation

```sh
roster recommend --task-class fix --difficulty 3
```

The shell equivalent is `/recommend fix --difficulty 3`. Both use the same
route selector as an opted-in run and print the chosen model, profile,
`source=evals` or `source=prior`, declared limits, and why. They use local
history only and never fetch leaderboard scores or PR evaluations.
Without a requested difficulty, the routing baseline is 2.

Each selected route also explains local human evidence by model, seat, task
class, and effort: distinct sample count, accepted/derived-rejected counts,
acceptance rate, latest known evaluation timestamp, and age in whole days.
The `origin` is `local-human-evaluations` or `none`; `source` still tells whether
the decision used evaluations or a capability prior/fleet hint. A recorded
defect still derives a reject without rewriting the human verdict.

No samples means an **unknown** acceptance rate, not measured zero. Fewer
than three samples or insufficient median rated difficulty emits an
`insufficient-qualifying-evidence` warning; effort groups and seats never pool
to meet the threshold. Missing/invalid/future timestamps leave recency unknown.
Recency is descriptive only: this change adds no stale-sample cutoff or ranking
weight. It does not infer hardware, token usage, throughput, or quality scores,
probe endpoints, or change saved configuration. This implements FEATURE_SPEC
sections 5.2 and 5.6 while respecting sections 3 and 7.

When no profile qualifies, output reports insufficient data and the saved
configuration default as information, **not** an automatic fleet choice.
Stats and the internal learning helper still expose evidence for other
models; displaying history does not register them.

## Execution remains explicit

```sh
roster run --issue N --auto-model
```

The shell equivalent is `/run N --auto-model`. The flag permits a
run-scoped endpoint/model choice even when a saved default already exists.
An empty fleet is refused before worktree creation; onboard or register
an endpoint first. Issue metadata supplies class and initial difficulty,
or a conventional issue-title prefix supplies class with baseline
difficulty 2. No recognized class or eligible profile leaves a truthful
deterministic stub, not a hidden fallback to an unregistered model.

The selected profile's endpoint/model and declared context feed the
planner and coder; the read-only reviewer follows that model. A routed
planner cannot switch to another model: malformed/mismatched plans must
be repaired within its existing budget or fail. The route and reason are
printed, but `.roster/config.yml` is never rewritten. Outside this explicit
routing path, existing task-specific [feedback selection](NEXT.md) remains
separate from fleet routing.

All configured profile API keys are excluded from test children and the
publisher when switching endpoints. Model inference still resolves only
the selected key name through the existing environment/vault path.
Routing grants no tools, App identity, policy, publication, merge, or
deploy capabilities. Internet remains a stored-only preference.

Run `node --test tests/route.test.mjs tests/builtin.*.test.mjs` for threshold,
prior, context-boundary, missing-profile, opt-in, and saved-default
regressions.
