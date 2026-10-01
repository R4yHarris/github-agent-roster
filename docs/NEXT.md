# The next task uses the last retrospective

The first task is a baseline, not proof of model capacity. When the next task
has the same `task_class`, the planner reads local human evaluations and joins
available contracts/run evidence through [feedback.mjs](../src/planner/feedback.mjs).
Both the deterministic stub planner and the configured planner use this step.
Offline draft asks also reuse local model history; no extra chat, queue, or
runtime is created.

## Selection before coding

- An explicit nonempty `TASK.md` model is preserved.
- Otherwise a [recommendation](LEARNING.md) with `n >= 3` and sufficient median
  difficulty supplies the model, known effort, and accepted-time estimate.
  `ESTIMATE.md` identifies that recommendation and its sample count. It does not
  replace an effort-specific recommendation with timing from another effort.
- Below that threshold, config (then `ROSTER_MODEL`) stays the default.
- If neither a task model nor a config/environment model exists, even **one
  accepted evaluation** can carry its model into the next stub task as a
  baseline. This is continuity, not a capacity recommendation: the default
  estimate stays 15 minutes until timing evidence qualifies. Rejected or
  excellence-failed deliveries cannot seed that baseline.

The coder uses the selected model and known effort; the planner's own model and
AI-Run identity do not change retroactively. An empty endpoint remains a stub:
history does not enable hosted inference, model calls, edits, tests, or
publication. The upfront [fleet `--auto-model` path](ROUTING.md) instead
locks a registered profile/model before planning, so feedback or a model
response cannot silently change its chosen endpoint/model. The per-task
selection above remains available outside that explicit routing path.

## Prior feedback

If a matching evaluation exists, the second task's generated `CONTEXT.md`
may contain `## Prior feedback` on the complex feat>=4 context path. Ordinary
minimum packs omit this model input while retaining factual memory/evaluation
records in the harness. When supplied, it summarizes the latest human verdict
and copies the last nonempty reject/rework comment for the same task class,
in append order. A newer acceptance does not erase that earlier lesson.
With only acceptances, the section explicitly says there is no reject/rework
comment yet. Other task classes are not mixed in.

Comments are quoted as data, not treated as policy or tool permissions.
The existing memory redactor is shared with feedback: known environment secret
values, private-key blocks, common token formats, credential assignments
(including quoted values), and URL credentials are removed before copying.
Do not put secrets in retrospective comments. Original human evaluation files
are never rewritten. Secret-like model metadata fails explicitly rather than
being copied into a task.

Prior feedback is a required section in the existing bounded context pack.
If it cannot fit alongside required instructions, increase `seat.context_chars`;
the run fails rather than silently losing the retrospective. Skills, memory,
research, principal limits, and protected task artifacts retain their existing
behavior. Missing history is normal; malformed JSONL and exporter failures are
errors, not silent routing fallbacks.

Run `node --test tests/feedback.test.mjs tests/context.test.mjs tests/planner.test.mjs`.
See [estimation](ESTIMATION.md), [human retrospectives](RETRO.md), and
[model capacity](LEARNING.md).
