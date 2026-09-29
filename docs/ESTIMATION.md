# Estimate before work

An estimate is story-point style feedback, not a delivery promise. The first
task has no history; compare its estimate with human-reported actuals and
refine the next estimate. Tests passing is not a human acceptance decision.

The planner writes these fields below the title in [TASK.md](../templates/sdlc/TASK.md):

```text
difficulty: 2
estimate_min: 15
task_class: fix
model:
```

Difficulty is an integer from 1 to 5, minutes a nonnegative integer, and class
one of `feat`, `fix`, `docs`, or `test`. Missing values default to difficulty 2
and 15 minutes. Class follows a recognized title prefix, otherwise `feat`.
An empty model uses the recommendation already selected by `--auto-model`, or
config (`ROSTER_MODEL` when config is empty). An explicit task model selects
the coder model, not the planner model. With no endpoint, it is still a stub.
The LLM planner may supply these optional fields; invalid values are rejected.

Before the coder loop, [estimate.mjs](../src/runtime/estimate.mjs) reads the
issue repository's optional `.roster/evals.jsonl` and `.roster/runs/*.jsonl`,
not the roster installation's history when those roots differ. Evaluations
can carry `model`, `task_class`, and `minutes` directly; older evaluations can
join run metadata by SHA or session. Missing history is normal. Malformed
history is an error with file and line context.

With at least three distinct, timed human evaluations for the same model and
task class, use the median minutes of the **accepted** runs. Rejected/reworked
runs count toward the evidence threshold, never toward the accepted timing
median. No accepted timings means keep the task/default estimate. Corrections
replace the previous evaluation for their target; duplicate lines are not
extra evidence. Half-minute medians round up to keep integer task minutes.

The resulting fields are written back to `TASK.md`, and `ESTIMATE.md` records
the estimate, source, and sample counts in the worktree **before** coding.
The coder may read but not rewrite either document. They are rechecked after
coding and before publication; generated estimates are not staged as app code.
No model usage or elapsed time is fabricated, and no evaluation is written.

Run `node --test tests/estimate.test.mjs tests/planner.test.mjs tests/builtin.test.mjs`.
