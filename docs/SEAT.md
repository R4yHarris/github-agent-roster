# One builtin SWE coder seat

A coder is one bounded principal executing one TASK.md, not a chat, worker
swarm, second queue, or new runtime. The same
[`runCoder`](../src/seats/coder.mjs) implementation serves the issue pipeline,
the offline demo, and an already-planned standalone task.

## Ordered contract

The coder awaits these stages in order:

1. **Principal:** load immutable coder conduct and fixed deny rules.
2. **Context:** save the ordered, bounded CONTEXT.md pack, including task
   acceptance/scope, instructions, planner-supplied prior feedback, skill
   previews, and recent seat memory.
3. **Research:** save the read-only RESEARCH.md inventory before edits, with
   at most one optional model summary.
4. **Skills:** load only requested task skills and verify their previews still
   match the context snapshot. Changed skills stop the seat, not widen context.
5. **Tool loop:** deterministic stub or bounded configured LLM, using only the
   five offered worktree tools. Configured final tests run unless task
   frontmatter explicitly says `tests: none`.
6. **Memory:** append factual, compact, credential-redacted notebook evidence.
7. **Excellence:** inspect the actual diff, tests, secrets, model, and turns.
8. **Result:** write RESULT.md with checks passed or the first failure, recorded
   stages/model/turns, and a summary. Never publish from the coder itself.

CONTEXT.md must preview skills to honor its pack contract; the post-research
skills stage validates the same selection rather than injecting a second,
unbudgeted copy into the model. A planner's task model is respected; otherwise
the configured model or `ROSTER_MODEL` is used. Estimation history is not rerun.

The result exposes `stages`, paths to the context/research/result artifacts,
`tests`, `testsSkipped`, `turns`, `model`, `usage`, `research`, and `excellence`.
Research usage joins coder usage; loop and research turns are separate.
Configured runs expose the existing contracts AI-Run without inventing unknown
counts. Private before/after snapshots are in-process evidence, not source
bodies or a persistent board.
Duplicate or malformed test declarations fail during preparation rather than
silently waiving verification; `tests: required` explicitly retains the default.

## Existing-task command

From the prepared worktree root, with TASK.md and AGENTS.md already present:

```sh
roster run --seat coder --runtime builtin
```

This command does not contact GitHub for an issue, invoke a planner, create
a worktree, rewrite TASK.md, or create RECIPE.yml. It uses the roster
installation's config, principal, skills, and memory. The pinned
[contracts dependency](DEPENDENCY.md) must resolve. Do not read an `.env`
through coder tools: a trusted launcher may provide `AI_TASK`/`AI_SESSION`;
otherwise local opaque IDs are generated. Opt-in `.roster/runs` recording
includes the gate outcome so operational failures cannot look like accepted
runs to [feedback-based routing](ROUTING.md).

`--auto-model` and `--publish` belong to the issue pipeline and are not accepted
on the standalone command. Review a passing run before the explicit App SDK
handoff. The command never creates a draft PR or publishes by itself.

The existing `roster run --issue N` and shell `/run N` still run planner then
coder. For compatibility, `--issue N --seat coder` still aliases that pair;
only the **no-issue** form above runs an already-planned coder alone.

## Stub and failures

With no endpoint, the stub derives its summary from the task title and checks.
It writes context, research, notebook, and result evidence, but **no application
code diff and no tests**. Its excellence report is unverified/failed, not a
completed software change; a normal offline stub exits zero.

Preparation, model, memory, test, and gate failures are reported with the stages
completed so far. If the worktree can safely accept a new RESULT.md, the harness
writes it before rejecting, even for failed setup. Existing artifacts are never
overwritten, and filesystem failures are explicit. No failed configured run
can publish. See [principals](PRINCIPALS.md), [context](CONTEXT.md),
[research](RESEARCH.md), [skills](SKILLS.md), [memory](MEMORY.md),
[tools](TOOLS.md), and [excellence](EXCELLENCE.md).
