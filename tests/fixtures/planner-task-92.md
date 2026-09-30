# Task: Add a one-line Status section to README.md

## Original Ask
Add a one-line Status section to README.md

## Scope
Edit `README.md` only. Add a short "Status" section consisting of a heading
(`## Status`) and exactly one line of body text describing the current project
state (e.g. `Active — under development.`). The section must be a single
non-empty line of content following the heading, placed in a sensible location
(typically near the top, after the title/intro, or at the end).

No other files, sections, or wording changes are in scope.

## Acceptance Checks
- `README.md` contains a heading exactly matching `## Status`.
- The `## Status` heading is immediately followed by exactly one non-empty line of text.
- `git diff --name-only` lists only `README.md` (no other files modified).
- `node --test` exits 0.
- Markdown remains valid: the status line is a single line (no trailing blank line inside the section before the next heading).

## Allowed Files
- `README.md`

## Metadata
- task_class: docs
- difficulty: 1
- estimate_min: 8
