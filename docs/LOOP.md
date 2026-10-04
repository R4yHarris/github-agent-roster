# Roster loop contract

The model proposes the next action. The harness decides whether that action is still live.

## Terminal states

A seat ends on the first of these. None of them are a model sentence.

- `verified`: the allowed-file check passed on the current file hashes.
- `baseline`: an in-scope check passed and every remaining failure is outside Allowed Files.
- `docs-checked`: the only allowed file is documentation and the required note is present.
- `repeat`: the same tool name, arguments, and target hash occurred twice.
- `budget`: turn, token, or repair budget is exhausted.
- `host`: the endpoint timed out. Do not start another coder turn in the same run.

## Rules

1. A passing check is bound to the file hash. Do not run it again unless an allowed file changes.
2. A baseline failure is reported once, then the seat returns. Remediation is a new session.
3. A docs task does not copy `node --test` into required evidence. The reviewer receives `tests: skipped-docs`.
4. `edit_file` compares normalized line endings. Two missed anchors append or replace a named section. They do not rewrite the file.
5. Search results may have an empty snippet. That is a result, and the next legal call is `web_fetch` on one returned https URL.
6. The planner writes checks that the harness can execute. A prose check is not a command.
