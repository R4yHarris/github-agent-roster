# Roster loop contract

The model proposes the next action. The harness decides whether that action is still live.

## Terminal states

A seat ends on the first of these. None of them are a model sentence.

- `verified`: the allowed-file check passed on the current file hashes.
- `baseline`: an in-scope check passed and every remaining failure is outside Allowed Files.
- `docs-checked`: the only allowed file is documentation and the required note is present.
- `repeat`: the same tool name, arguments, and target hash occurred twice.
- `budget`: turn, token, or repair budget is exhausted. A test repair budget starts from task difficulty (1, 2,
  or 4); each repair whose failure is new and whose failing-test count did not rise earns one more, up to 12.
  A failure that matches any earlier one (including oscillation) is steered once toward a different approach and
  stops the context on its second repeat (`Test repair stalled`). A progressing context past half its declared
  `context_max` is handed to a fresh context instead of being extended (`Test repair handoff`). In a builtin run,
  an exhausted turn or repair budget, a stalled or handed-off repair (or
  a repeated denied action) ends this coder context only: the harness starts up to two fresh coder contexts
  (`Perspective escalation N of 2`), on a different eligible fleet profile when one exists. Worktree edits are kept,
  and the last failure is passed in as evidence, with each earlier context's model, stop reason, and failing-test
  count so the next one does not retry them. Recorded scope expansions and regression-repaired tests outside
  Allowed Files stay in scope for the next context, so its gate does not reject an earlier context's legitimate
  repair. A resumed run restores the same recorded scope from archived results for files still changed in the
  worktree. A hard security denial is never escalated.
- `host`: the endpoint timed out. Do not start another coder turn in the same run.

## Rules

1. A passing check is bound to the file hash. Do not run it again unless an allowed file changes.
2. A baseline failure is reported once, then the seat returns. Remediation is a new session.
3. A docs task does not copy `node --test` into required evidence. The reviewer receives `tests: skipped-docs`.
4. `edit_file` compares normalized line endings, then accepts a unique match that differs only in indentation (re-indenting the replacement). A miss returns the closest current text to copy. Two missed anchors append or replace a named section. They do not rewrite the file.
5. A tool usage denial stops the seat only when the same call fails the same way three times. A successful write resets the count, because distinct misses separated by progress are not a loop.
6. Search results may have an empty snippet. That is a result, and the next legal call is `web_fetch` on one returned https URL.
7. The planner writes checks that the harness can execute. A prose check is not a command.
