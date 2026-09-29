# Task-selected SWE skills

Skills live in this repository's [skills directory](../skills/), not a Hermes
home or another runtime. They are Markdown procedures, not code, permissions,
or additional seats. Each describes when to use it, steps, and a stop condition.

The bundled coder procedures are:

- [implement-task](../skills/implement-task/SKILL.md)
- [run-tests](../skills/run-tests/SKILL.md)
- [read-before-write](../skills/read-before-write/SKILL.md)
- [small-diff](../skills/small-diff/SKILL.md)
- [result-report](../skills/result-report/SKILL.md)

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
skill within its budget. The full files remain available as documentation.
Estimation preserves the frontmatter and still derives difficulty, class,
model, and minutes from the task header/history. Skills cannot override the
[principal](PRINCIPALS.md), allowed paths, tool set, or publication policy.
