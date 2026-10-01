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
   When `tools.run_test` is false, required-test tasks fail before inference;
   the permission does not silently waive acceptance checks.
6. **Memory:** append factual, compact, credential-redacted notebook evidence.
7. **Excellence:** inspect the actual diff, tests, secrets, model, and turns.
8. **Result:** write RESULT.md with checks passed or the first failure, recorded
   stages/model/turns, and a summary. Never publish from the coder itself.

CONTEXT.md must preview skills to honor its pack contract; the post-research
skills stage validates the same selection rather than injecting a second,
unbudgeted copy into the model. A planner's task model is respected; otherwise
the configured model or `ROSTER_MODEL` is used. Estimation history is not rerun.

### Minimum enforced senior-team loop

Ordinary tasks use a short harness-selected pack: issue Ask, TASK outcome/
allowed files/checks (plus any explicit Scope constraints), and the
`read-before-write` / `small-diff` skills. Difficulty1-2 always take this path;
docs tasks do so at every difficulty. Extra skill selectors and principal/
AGENTS/memory/feedback inputs do not enlarge it. No RESEARCH file or research
model request is generated. Memory output remains factual. Only `feat` tasks
with difficulty >=4 may use RESEARCH and requested implementation skills,
including `implement-task`; other classes do not unlock that path.
Classified slices always use the minimum pack, even at feat difficulty4+;
feature and initiative runs plan without invoking this coder at all.

The team contract is enforced by code, not a longer role prompt: scoped reads/
writes, `run_test` unless explicitly waived, excellence for paths/secrets/tests,
read-only reviewer, App-only reviewed publication, and a human eval hint.
No task metadata grants permissions or generates AI-Eval.

Difficulty1 docs tasks whose application allowlist is exactly README.md have
an additional enforced tool contract: only TASK.md and README.md may be read;
README.md must be successfully written before tests or final completion.
Directory listing, repository search, RESEARCH.md and planner/test fixtures
are denied, not just omitted from the prompt. The model receives only
read_file, write_file and permitted run_test with exact file-path schemas.
Other classes, difficulties and broader scopes retain their existing tools.
Allowed reads can precede the write to preserve read-before-write; disabling
write_file or omitting the edit fails explicitly, even when tests are waived.

Planner scope is limited to files explicitly listed or named by the human Ask.
An Ask with no file scope needs clarification; it never becomes an invented
`**/*` allowance. A slice with inferred scope prints its validated TASK summary
and continues to coder in the same run. Only `--confirm` pauses after the
summary; no second `/run` or `--auto` is required. Multiple explicitly
stated Outcomes classify as a feature and produce child issue drafts in
PLAN.md instead. An already valid cached slice TASK/recipe bypasses the
planner and starts coder; a feature/initiative cannot reuse it to bypass
planning-only execution. See [Ask classification](SDLC.md#agile-mapping).

The result exposes `stages`, paths to the context/research/result artifacts,
`tests`, `testsSkipped`, `turns`, `model`, `usage`, `research`, and `excellence`.
Configured runs also expose `testRepairs`, `repairFiles`, and (on exhaustion)
`repairBudgetExhausted`. Failed tests are repaired up to four times after the
initial run, with a fresh tool-turn budget per attempt and redacted diagnostics.
The harness grants only regular failing test files identified in Node failure
locations in addition to TASK scope; reviewer and publication use that same
recorded scope. TASK.md itself remains unchanged. Only green tests can pass;
budget exhaustion writes a failing review without reviewer inference.
Timeouts and denied tool paths still stop with an unverified result.
Research usage joins coder usage; loop and research turns are separate.
Configured runs expose the existing contracts AI-Run without inventing unknown
counts. Private before/after snapshots are in-process evidence, not source
bodies or a persistent board.
Once RESULT.md is written, the same-process [reviewer seat](REVIEW.md)
checks the task and diff without coder tools and writes REVIEW.md.
An HTTP timeout writes `Outcome: timed out (unverified)` and an unverified
summary, never a success claim. The reviewer writes `Verdict: fail` with the
coder HTTP timeout reason and explicitly says review was not completed; it
does not request a model verdict for timed-out work.
Duplicate or malformed test declarations fail during preparation rather than
silently waiving verification; `tests: required` explicitly retains the default.

## Existing-task command

From the prepared worktree root, with TASK.md and AGENTS.md already present:

```sh
roster run --seat coder --runtime builtin
```

For a classified slice, this command does not contact GitHub for an issue,
invoke a planner, create a worktree, rewrite TASK.md, or create RECIPE.yml.
A broad feature/initiative TASK is classified before coder and produces only
PLAN.md; its existing TASK and source files stay unchanged. It uses the roster
project private config when available, then installation settings; principal,
skills, and memory still come from the roster installation. The pinned
[contracts dependency](DEPENDENCY.md) must resolve. Do not read an `.env`
through coder tools: a trusted launcher may provide `AI_TASK`/`AI_SESSION`;
otherwise local opaque IDs are generated. Opt-in `.roster/runs` recording
includes the gate outcome so operational failures cannot look like accepted
runs to [feedback-based routing](ROUTING.md).

`--auto-model`, `--publish`, and `--skip-review` belong to the issue pipeline and are not accepted
on the standalone command. Review a passing run before the explicit App SDK
handoff. The command never creates a draft PR or publishes by itself.

For slices, `roster run --issue N` and shell `/run N` run planner, coder,
then reviewer. Feature/initiative runs stop after PLAN.md, and `clarify`
stops before any seat. For compatibility, `--issue N --seat coder` still aliases the
full sequence;
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
