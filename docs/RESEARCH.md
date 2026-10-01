# Research before editing

Difficulty1 `docs` tasks use the [minimum context path](SEAT.md#minimum-context-for-easy-docs):
they do not create/load RESEARCH.md or make the optional research model call.
Read-before-write happens directly through scoped tools, with small-diff and
the usual tests/path/secret excellence guards. Other classes/difficulties keep
the research sequence below. before editing

The [coder seat](../src/seats/coder.mjs) awaits a
[read-only research step](../src/runtime/research.mjs) after its context pack
and before entering the tool loop. No model `write_file` can run before this
step finishes.

Research rereads TASK.md and verifies it still matches the context snapshot.
It lists the task's allowed paths and resolves explicit files and `directory/**`
patterns using the same guarded `list_dir`/`read_file` tools as the coder.
Protected paths and symlinks are never followed.

At most **eight distinct candidate files** are read, with **200 lines per
file**. The mandatory TASK.md control read is separate from that source-file
budget. Each report excerpt is additionally capped at 8000 characters, with
an explicit truncation marker. Missing files/directories are recorded as gaps,
not silently treated as implemented; additional allowed files beyond the
budget are explicitly marked uninspected.

The harness creates `RESEARCH.md` in the worktree, containing:

- What the task asks, including acceptance checks.
- Allowed paths and what exists, with bounded source excerpts.
- Missing files, unread binary content, budget omissions, and unverified checks.

Research never invokes `write_file`, changes application files, or runs tests.
The report is a protected harness artifact and is excluded from publication.
An existing report is not overwritten. A task change, unsafe path, or file
error other than an expected missing candidate stops the seat before edits.

## Stub and configured modes

Without an LLM endpoint the inventory is entirely deterministic and offline.
With `llm.base_url` configured, one tool-free chat call may summarize up to
8000 characters of the report. The inventory is already on disk before this
call. A failed or malformed summary is recorded explicitly, without raw
endpoint diagnostics, and the inventory is retained so the coder can continue.
The model cannot call tools during research.

The configured tool loop is instructed to read the report. Research usage is
combined with loop usage for the coder's AI-Run; missing usage stays unknown,
not zero. `research.turns` separately records the optional single request;
the existing tool-loop turn budget and `result.turns` retain their meaning.
