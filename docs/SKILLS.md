# Task-selected SWE skills

Skills live in this repository's [skills directory](../skills/), not a Hermes
home or another runtime. They are Markdown procedures, not code, permissions,
or additional seats. Each describes when to use it, steps, and a stop condition.

The bundled baseline coder procedures are:

- [implement-task](../skills/implement-task/SKILL.md)
- [run-tests](../skills/run-tests/SKILL.md)
- [read-before-write](../skills/read-before-write/SKILL.md)
- [small-diff](../skills/small-diff/SKILL.md)
- [result-report](../skills/result-report/SKILL.md)

FEATURE_SPEC **5.2** category packets are also installed: [research](../skills/research/SKILL.md),
[implement](../skills/implement/SKILL.md), [test](../skills/test/SKILL.md),
[review](../skills/review/SKILL.md), [incident](../skills/incident/SKILL.md), and
[docs](../skills/docs/SKILL.md). Select a category explicitly in TASK frontmatter
when its bounded procedure is needed. This is staffing/context guidance, not
primary question/incident execution, internet access or a principal change.
A coder receiving the review packet is not the independent reviewer.

The [TASK template](../templates/sdlc/TASK.md) requests all five. A task can
select a subset in initial frontmatter:

```yaml
---
skills: [read-before-write, implement-task, run-tests]
---
```

An indented two-space list under `skills:` is also supported. Empty or absent
skills mean none are requested; there is no implicit directory-wide loading.
Names must be distinct lowercase letters/digits/hyphens, starting with a letter.
Paths, duplicate fields/names, and malformed lists fail clearly.

[`loadSkills({ repoRoot, task, skillsPath })`](../src/runtime/skills.mjs) opens
only the named `skills/<name>/SKILL.md` files, in task order. `paths.skills`
can select another directory inside the roster installation, never an external
home. Unknown requested names, empty/oversized files, and symlink paths fail
before the coder's model/tool turn. An unrequested malformed skill is ignored.
Each skill file must be a regular UTF-8 Markdown file of at most 64 KiB.

The [context pack](CONTEXT.md) includes names and the first 40 lines per selected
skill within its budget. Minimum coder contexts retain the existing two baseline
packets (read-before-write/small-diff), plus every explicitly selected spec
category in task order. Their category procedures are required context, not
silently discarded by the minimum policy. Full contexts retain existing task
selection, and a category-containing skill section is required rather than
budget-optional. If required context cannot fit, preparation fails before
inference/product edits; the operator must raise the bounded context budget or
re-scope, never silently omit the declared procedure. Baseline-only template
selection and default context budgets remain unchanged. Unselected packets are
not read; selected missing/malformed/symlink packets fail through the existing
loader. Category packets are under 40 lines, so their stop/boundary guidance is
part of the actual delivered preview. The full files remain available as documentation.
Estimation preserves the frontmatter and still derives difficulty, class,
model, and minutes from the task header/history. Skills cannot override the
[principal](PRINCIPALS.md), allowed paths, tool set, or publication policy.
