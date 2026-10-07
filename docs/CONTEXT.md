# One-task context pack

The builtin coder saves [CONTEXT.md](../src/runtime/context.mjs) in its issue
worktree before the tool loop. This is the exact initial context sent to the
model, not a separate or more complete transcript. The loop also sends a fixed
request to execute the task. That request is a numbered checklist with one item
per TASK.md acceptance check, followed by the rules
([#294](https://github.com/R4yHarris/github-agent-roster/issues/294)).
Subsequent responses and tool results are not part of this initial pack.

The order is:

1. The installation's [coder conduct](../principals/coder.md).
2. TASK.md, including its title, acceptance checks, allowed paths, and Ask.
3. The coding rules from the worktree's AGENTS.md. The
   [seat rules](../src/runtime/seat-rules.mjs) drop publication, App
   credential, and session-metadata sections and items, because the harness
   publishes after review. A note marks the omission.
4. [Prior feedback](NEXT.md), when the planner found a same-class human evaluation,
   with credential redaction and quoted retrospective comments.
5. Only task-named skills, in task order, with their names and first 40 lines.
6. The last 20 lines of this seat's memory, labeled as data, not instructions.
7. Rule layers, in precedence order with their source labels: human policy
   (enforced by tools), org rules, repo rules, area rules, task rules, and the
   seat principal ([#301](https://github.com/R4yHarris/github-agent-roster/issues/301)).
8. [Conventions](../src/runtime/conventions.mjs) derived deterministically from
   the worktree: language and module system, package manager and runtime
   dependency count, test runner, directory and file shape, file and export
   naming, lint/format config, the largest module and test, and the five
   most-imported internal modules. Only names and counts are read; secrets,
   managed files, dot directories, `vendor/`, and `node_modules/` are skipped.
   The result is cached per worktree and refreshed when `HEAD` changes.
   Layers and conventions are optional: under a tight budget they truncate
   after memory, never before required context.
9. The relevant file/path list from TASK.md, not a repository inventory.

TASK.md may select skills using frontmatter such as
`skills: [implement-task, run-tests]`, or an indented list under `skills:`.
Absent frontmatter means no skills are requested. Names must be distinct
lowercase names; a missing requested skill fails clearly.

`seat.context_chars` in [config](../roster.config.example.yml) is an optional
positive character budget, defaulting to **8000**. No tokenizer is bundled;
this is a character limit, not an estimate of model tokens. `llm.context_max`
continues to describe model capacity for AI-Run and is not reinterpreted.
The limit includes headings and omission markers in CONTEXT.md. Required
conduct, TASK.md, AGENTS.md, prior feedback, and relevant paths are never silently cut: if
they do not fit, the turn fails and asks for a larger budget. Skill excerpts
and then older memory may be omitted on whole-line boundaries, explicitly
marked `[Omitted by context budget]`.

The pack never opens file references from the task. In particular it does not
include `.env`, PEM, policy, or vault file bodies. Protected allowed paths
are rejected by the task parser; symlinked context inputs are refused.
The harness exclusively creates CONTEXT.md, refuses to overwrite an existing
one, denies coder writes to it, and excludes it from publication.
