# Ask: <short software outcome>

Use this as the GitHub issue body. Replace placeholders with concrete facts;
keep unresolved decisions visible rather than inventing requirements.

## Problem and desired outcome

- User or caller: <who needs this>
- Current behavior: <what happens today>
- Desired behavior: <what should happen instead and why>

## Reproduction and context

- Input and setup: <minimal reproduction, fixture, or example>
- Actual result: <observable output, error, or side effect>
- Expected result: <exact output, error, or side effect>
- Relevant code, tests, or documentation: <repository paths or links>

Do not include credentials, private keys, tokens, or environment-file contents.

## Requirements

- **R1:** <normal-case behavior with concrete inputs and expected results>
- **R2:** <invalid-input or failure behavior, including forbidden side effects>
- **R3:** <existing behavior or interface that must remain compatible>

## Scope and constraints

- In scope: <bounded change>
- Out of scope: <non-goals>
- Allowed files or areas: <implementation, tests, and documentation>
- Protected files or areas: <must not change>
- Environment: Node 20+ ESM; no new runtime dependencies unless explicitly
  authorized by the task.
- Verification: Node's built-in runner, `node --test`.
- Execution: one coder seat in one assigned worktree; no merge or deploy rights.
  Human-owned policy is not part of the change.

## Success examples

| Requirement | Given / input | When / action | Then / observable result |
| --- | --- | --- | --- |
| R1 | <normal input> | <operation> | <exact result> |
| R2 | <invalid input or failure> | <operation> | <error and absent side effects> |
| R3 | <existing supported case> | <operation> | <unchanged result> |

Include units, thresholds, and a reproducible measurement if performance or a
size limit is part of the ask; "fast" or "small" alone is not an acceptance check.

## Open questions

- <decision, who can resolve it, and which requirement it blocks; or "None">
