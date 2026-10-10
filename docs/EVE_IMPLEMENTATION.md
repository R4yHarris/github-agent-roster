# Eve implementation: first-wave delivery plan

Status: implementation initiated on 2026-10-10, not a claim that Eve is
complete or that her proposed performance targets have been measured.

The [research report](EVE_RESEARCH.md) was independently reviewed and
App-published in [#472](https://github.com/R4yHarris/github-agent-roster/pull/472).
The user subsequently authorized remote persistence and distributed
implementation. This document records the initial bounded rollout; GitHub
issues, PRs, reviews, and human evaluations remain the live delivery record.
It is not a second board.

## Scope and authority

Start with the source-grounded software-worker functions proposed in research
sections 7, 8, and 11. Keep the existing single-process planner -> coder ->
reviewer sequence per task. Eve is not a new runtime, a chatbot crew, a
GitHub principal, a claim of consciousness, or an autonomous policy owner.

This first wave implements two independent foundations:

1. Explicit, bounded acceptance-obligation continuity across coder handoff.
2. Offline paired evaluation of baseline and Eve observations.

Stable named-worker identity, curated-memory improvements, resource
experiments, inference-time alternatives, and council specialization require
later scoped work and evidence. An opt-in continuation feature and a synthetic
evaluation fixture cannot establish an integrated Eve system or empirical
software-delivery superiority.

## Remote delivery structure

| Issue | Deliverable | Prerequisites |
| --- | --- | --- |
| [#473](https://github.com/R4yHarris/github-agent-roster/issues/473) | Overall Eve implementation initiative and human-visible delivery record | Reviewed research #472 |
| [#474](https://github.com/R4yHarris/github-agent-roster/issues/474) | Opt-in, source-linked TASK acceptance continuation through the actual builtin issue recovery path | None |
| [#475](https://github.com/R4yHarris/github-agent-roster/issues/475) | Dependency-free offline paired pilot evaluator with an executable local JSON path | None |
| [#476](https://github.com/R4yHarris/github-agent-roster/issues/476) | Integrated first-wave baseline/continuation/evaluator validation | #474 and #475 |
| [#477](https://github.com/R4yHarris/github-agent-roster/issues/477) | Scoped identity, memory, resource, search, and council follow-up planning | #476 |

Issue bodies contain the exact planned files, acceptance checks, and
`Depends on:` links. The integration and later initiatives are not permission
to implement an entire epic in a coder turn.

Dependency readiness follows [MULTIAGENT](MULTIAGENT.md): issues must be
closed on GitHub. Passing tests, passing review, and a merged PR are not a
human acceptance evaluation and do not automatically close an issue.
Do not generate human `AI-Eval` records to advance the board.

## First-wave implementation boundaries

### Acceptance continuation

[#474](https://github.com/R4yHarris/github-agent-roster/issues/474) targets the
existing config, checklist, context, coder loop, and builtin recovery path.
The proposed `seat.evidence_workspace` boolean defaults off.

The feature must carry TASK-derived check identifiers, exact obligations,
honest statuses, and bounded redacted evidence references. It must not
replace TASK, widen authority, promote missing evidence, store hidden
reasoning, or certify changed bytes using an obsolete passing check.

A public issue-run regression must force a real coder handoff and verify
what the next coder receives. Calling a helper twice without wiring the
actual builtin recovery path is not sufficient.

### Offline evaluator

[#475](https://github.com/R4yHarris/github-agent-roster/issues/475) targets a
pure evaluator, a thin local script, fixtures, tests, and directly related
documentation. It is independent of runtime continuation changes.

Pair baseline/Eve records by task and matched controls. Separate deliberate
behavior changes from controls that must stay identical. Report multiple
trials separately from independent task counts. Keep human acceptance
distinct from automated checks and reviewer verdicts.

Missing usage, energy, cost, hardware, latency, and calibration stay unknown.
A small synthetic fixture must report incomplete pilot coverage. It must
not create state, read raw machine history, call a model, publish, or imply
that the research target of 40 held-out tasks plus 20 fault cases was run.

## Distributed execution without a second runtime

Initial dispatch uses two coordinator task sessions that call Roster's
existing `runBuiltinIssue` entry point. The sessions do not themselves edit
application code or simulate planner/coder/reviewer chats. Within each issue,
the builtin seats run sequentially in a dedicated issue worktree.

- #474 is assigned to the already registered private `spark-dsv41` profile.
- #475 is assigned to the already registered private `default` Qwen profile.
- Both endpoints responded to a bounded model-inventory probe and advertised
  the registered model before dispatch.
- Each endpoint declares concurrency 1. The `default` and
  `rtx-3090-qwen` profiles share an endpoint, so they are not counted as two
  independent capacities.
- No non-private model endpoint was selected for this first wave.

These are dispatch facts, not quality or throughput measurements. Model
availability is not proof of tool competence, context reliability, or safe
execution. Do not infer energy from GPU-seconds or guess endpoint usage.

Each run uses a bounded turn/request budget and actual response-backed
seat attribution. Automatic publication is disabled during initial execution
so the coordinator can inspect the result and independent review before
calling the App publisher. No model switch, additional endpoint, private
configuration rewrite, or second patch writer is implicit in this dispatch.

## Verification and publication

1. Start each slice from fresh `origin/main`; retain its isolated worktree.
2. Validate the actual requirement with targeted Node 20 tests, including
   public-path integration rather than helper-only assertions.
3. Preserve red/failure evidence and use the existing bounded repair and
   perspective-escalation paths. A stuck model or malformed review is not a
   successful implementation.
4. Inspect the diff and per-check review. Re-run relevant checks after the
   last edit. Never weaken checks or reinterpret unknown as pass.
5. Publish only eligible reviewed changes, from the owning feature
   worktree root, through the required contracts App publisher and
   `--merge-when-green`. Keep actual completed-seat model/usage attribution;
   do not overwrite it with the coordinator's model.
6. Verify the remote PR state and trusted checks. Leave issues open for
   human evaluation and closure.
7. Run the dependent integration slice only after its GitHub dependencies
   satisfy the existing readiness rule.

Publication of research #472 succeeded and both trusted checks passed.
The SDK's subsequent local checkout cleanup could not switch to `main`,
which is owned by another worktree. The remote merge was independently
verified; the original feature branch was retained and fast-forwarded to
the merged remote base. No duplicate commit or human-authenticated push
was used.

If a future App publication returns HTTP 422 for Checks permission, stop
and report the installation issue; never use a credential workaround.

## Verified recovery and available operations

The initial registered private-endpoint runs did not deliver application code.
One baseline verification path incorrectly applied a 20-second Node 20
aggregate test-file limit to healthy lifecycle fixtures; later attempts also
encountered stalled inference, restricted-read retries, and a rejected planner
revision. These are failed delivery attempts, not evidence of model quality,
successful seats, or Eve performance.

The tightly coupled verification fix was independently reviewed and merged
through the App in [#479](https://github.com/R4yHarris/github-agent-roster/pull/479).
It reuses the existing 15-minute code-verification bound for scoped and final
test files/processes, preserving selectors, explicit failures, cancellation,
scope checks, and acceptance assertions. Node 20 also bounds aggregate file
lifetime; this is not a separate two-minute subtest guarantee.

After preserving the plans and cancelling the unsuccessful local coder
requests, explicit GHCP implementation specialists recovered the two slices
with one writer per isolated worktree. They are implementation assistance, not
new Roster seats, a swarm runtime, or successful local-model measurements.
Independent code review and coordinator verification precede App publication.
No failed or cancelled seat is assigned fabricated completion or usage.

The offline evaluator for #475 is merged in
[#480](https://github.com/R4yHarris/github-agent-roster/pull/480), with both
trusted checks successful. Its Node 20 pilot tests pass 19/19; the coordinator
also verified byte-deterministic public CLI output and the golden JSON shape.
See [EVE_PILOT](EVE_PILOT.md) for the schema, library API, bounds, and limits:

```powershell
node scripts\eve-pilot.mjs --manifest tests\fixtures\eve-pilot.json
node scripts\eve-pilot.mjs --help
node --test tests\eve-pilot.test.mjs
```

The sample remains synthetic and `pilot-incomplete`. A successful comparison
command is not a passed pilot, measured acceptance, or human `AI-Eval`.
Human evaluation and closure of #474/#475 remain prerequisites for #476;
merged PRs and successful automated checks do not advance that authority gate.

Acceptance continuation for #474 is reviewed in non-draft
[#481](https://github.com/R4yHarris/github-agent-roster/pull/481).
Coordinator Node 20 verification passed 85 assertions with one existing Windows
symlink skip, plus 26 boundary assertions. Review caught and resolved an
evidence-prose elevation bug: only canonical host references cross the handoff,
not free-form instructions from the previous coder.

The first full CI run passed 1,573 assertions and failed only the unchanged
1,000-line test-layout gate. The new regression was extracted, unchanged, to
`tests/loop.acceptance.test.mjs`, with the justified one-file scope expansion
recorded on #474. Coordinator runtime/shard/layout verification passed 36/36.
The source-module naming and size gates were not weakened.

Publication hit the pinned contracts publisher's stale PR-head race on
subsequent pushes. The corrected code and verification documentation are
remotely persistent; the PR is ready, not draft, but merge is not claimed here.
A retry with no staged changes is rejected by that publisher, so it is not an
idempotent merge-resume path. Do not generate duplicate commits, rewrite the
vendor pack, or use human push/PR credentials to conceal that limitation.
Confirm the latest head and trusted checks before resolving the merge blocker.

## Spec trace and non-goals

This plan traces to [FEATURE_SPEC](FEATURE_SPEC.md) sections 5.2
(staffing/capacity), 5.3 (bounded slices/dependencies), 5.4 (execution),
5.5 (review/gates), 5.6 (honest learning/evaluation), and 5.8 (operation),
with App authority from 5.1. It honors sections 3 and 7.

No Kanban DB, runtime dependency, vendor rewrite, policy edit, workflow
edit, global/shared private memory, autonomous weight training, or default
production deployment is included. [STATE](STATE.md) ownership and raw-history
restrictions remain binding. Future council work must earn value rather than
maximize the number of agents for its own sake.
