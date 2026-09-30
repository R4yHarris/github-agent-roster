# The SWE delivery feedback loop

Roster is a delivery harness, not a chat UI. One issue moves through a builtin
planner, coder, and reviewer sequentially in one worktree. GitHub Issues and PRs are the
board and forge; task artifacts, measured outcomes, and the human retrospective
connect one delivery to the next.

## Authority comes first

A [principal](PRINCIPALS.md) limits what a seat may do. The conduct file, configured
tools, task scope, and hard read/write boundaries apply before execution. Skills
and model suggestions do not grant access to secrets, policy, workflows, or
additional capabilities.

The authenticated GitHub App, not the model or seat name, is the publishing
principal. Reviewed publication uses the pinned contracts SDK and human-owned
policy; required checks and repository protections still apply. See the
[GHCP bridge](GHCP.md). There is no second queue, concurrent swarm, or implicit
deployment.

## Estimate before work

The planner writes `TASK.md` with `difficulty`, integer `estimate_min`,
`task_class`, and a model selection. `ESTIMATE.md` records the estimate and its
evidence before coding. The [estimation guide](ESTIMATION.md) defines the
initial difficulty 2 / 15-minute baseline and when accepted-run timing can
replace it. These are story-point style estimates, not delivery promises.

## Deliver with evidence

The coder receives a bounded context pack and task-selected skills, performs
the [research step](RESEARCH.md), and edits only through its configured,
[worktree-scoped tools](TOOLS.md). Tests run after the last edit unless the
task explicitly declares no tests.

The [excellence gate](EXCELLENCE.md) checks test evidence, changed-file scope,
detected secrets, and truthful model/turn reporting before `RESULT.md`.
A failed gate still produces an explicit failure report and cannot publish.
An offline stub writes demonstration artifacts without claiming implementation
or test execution.
The coder's redacted excellence failure reasons append to
`.roster/runs/runs.jsonl` as `defects`, alongside the gate outcome. A
publication-time recheck also appends newly discovered defects; reports
are not rewritten to erase failed verification.

## Review before publication

After the coder writes RESULT.md, the read-only reviewer examines its
acceptance checks and diff and writes [REVIEW.md](REVIEW.md). A failed review
preserves the work but blocks Roster-managed publication unless `--skip-review`
is explicit; the flag does not waive tests or human-owned policy. Human PR
review and AI-Eval remain separate. Review the delivered evidence yourself
before a direct SDK invocation, which does not enforce this in-process gate.
Every publish path supplies the real `--model` from config,
then `AI_MODEL`, then `ROSTER_MODEL`; no candidate means `set model`, not
`AI-Model: unknown`. This GHCP agent sets `AI_MODEL=GPT-6.1-Sol`.
The PR body includes `## Model`, `## Summary`, and how to test the change.
See the [reviewed publication example](GHCP.md).
Issue-run PR bodies link `Refs #N` rather than a closing keyword. The App
comments the verified PR URL and real coder model ID on the open issue;
only the human closes it after AI-Eval. GitHub Issues and PRs remain the
sole queue and forge, not a local Kanban database.

## AI-Run is machine evidence, not authority

Contracts owns the schema and trailer parser. An illustrative schema 1 record is:

```text
AI-Run: 1|local|served-model@-|h|1200/8192|300|roster-42-coder|issue-42
```

The fields are schema, provider, model/version, effort, reported input/context
tokens and capacity, reported output tokens, session, and task. Usage comes
from actual reports; missing values stay `-`, never estimated token counts.
Planner, coder, and reviewer keep separate identities and usage; the publishing SDK carries
the coder record on the code commit.

A configured model must retain its served ID. **An unknown model is a broken
trail, not a model value to recommend.** Model-free stub runs omit AI-Run.
The pinned contracts `v0.2.1` provider catalog encodes vLLM as `local`, not
the unsupported literal `vllm`; no contracts source is rewritten. See
[metrics](METRICS.md) for the exact fields. The SDK's required AI-Model
trailer also fails closed without a real model ID.

## AI-Eval belongs to the human

After inspecting the result, only the human records the [retrospective](RETRO.md):

```sh
roster eval roster-42-coder accept 3 n --minutes 18 --comment "Ship quality; retain the regression test."
```

Difficulty is the refined point estimate, minutes are actuals, and verdict
means ship-quality (`accept`), repeat this task (`rework`), or reconsider the
breakdown (`reject`). The command appends local JSONL; with one matching PR it
posts the compact human AI-Eval and Minutes lines. Free-text feedback stays
local. The coder and model have no evaluation-writing tool and cannot accept
their own work.

## The next task reads that history

[Learning](LEARNING.md) joins contracts exports, local runs, and human
evaluations. Stats expose sample counts, acceptance rates, and median minutes
and difficulty. Recommendations need at least three distinct samples and
enough observed difficulty; insufficient data explicitly reports the config
default. There is no hidden quality score. Recorded excellence failures count
as rejects even if tests passed, without rewriting the human evaluation.
Defects survive duplicate run reports and later evaluations: a recorded
secret-path or policy touch remains a reject for recommendations even if a
human later accepts the delivery. A passing gate never substitutes for the
human `roster eval ... accept` needed to count an acceptance.

For the same task class, the [next task](NEXT.md) uses qualifying model/effort
and timing evidence. Its context must include redacted **Prior feedback** when
a matching evaluation exists, including the last reject/rework comment.
The first task is a baseline; even one acceptance can preserve an otherwise
unconfigured baseline model, but it is not proof of capacity.

## Zero defects is the target, not the claim

Passing tests and checks are evidence, not a guarantee of correctness or an
automatic human acceptance. State failures, missing evidence, and remaining
uncertainty plainly. The aim is the next task at this difficulty with fewer
defects, using the last retrospective rather than repeating an unsupported
success claim.
