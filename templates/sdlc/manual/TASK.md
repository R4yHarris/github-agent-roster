# Task: <short software outcome>

Copy to the assigned worktree root. Replace placeholders and the example issue
number `42` before implementation. Resolve blocking questions on the issue.

- Issue URL: <current repository's issue URL>
- Task ID: `issue-42`
- Recipe: [RECIPE.yml](RECIPE.yml)
- Source ask: [ASSIGNMENT.md](ASSIGNMENT.md#ask)
- Seat / principal: `coder` / `coder`

## Objective

<One bounded outcome derived from the ask, not a broader redesign.>

## Scope

- May change: <explicit implementation, test, and documentation paths>
- Must not change: <protected paths and interfaces>
- Non-goals: <excluded work>
- Constraints: Node 20+ ESM, no new runtime dependencies unless explicitly
  authorized, one coder, no policy edits, no merge or deploy.

## Inputs and decisions

- Reproduction or fixtures: <concrete setup and inputs>
- Existing behavior to preserve: <compatibility contract>
- Resolved questions and assumptions: <issue comment links and decisions>
- Blocking questions: <None, or a decision that must be resolved before coding>

## Acceptance checks

- [ ] **AC-1 (R1, normal case):** Given <setup and exact input>, when <operation>,
  then <exact observable output and side effects>.
  Verify with <test file and test name, or a reproducible manual procedure>.
- [ ] **AC-2 (R2, failure case):** Given <invalid input or injected failure>,
  when <operation>, then <expected error and side effects that must not occur>.
  Verify with <test file and test name, including assertions for absent effects>.
- [ ] **AC-3 (R3, regression):** Given <previously supported input>, when
  <operation>, then <unchanged public behavior>.
  Verify with <test file and test name, or a reproducible manual procedure>.

Add or remove checks to match the actual ask. Every requirement needs a check;
every check needs an observable result and evidence. Include boundary values and
measurable thresholds where relevant. Do not weaken checks to match a solution.

## Verification plan

List the exact `node --test` commands covering the checks, preferably naming the
affected test files together. Use `node --test` for the full suite when required
by scope or acceptance. No other runner or dependency installation is implied.
For any manual check, specify setup, action, expected result, and evidence.

## Evidence

| Check | Command / procedure and test name | Actual result | Status |
| --- | --- | --- | --- |
| AC-1 | <verification> | Not run | Pending |
| AC-2 | <verification> | Not run | Pending |
| AC-3 | <verification> | Not run | Pending |

Record exit codes and test counts for automated checks. A skipped, unmatched, or
unrun test is not a passed check. Report failures and blockers explicitly.
Builtin tools cannot edit this generated task; report evidence in the final
summary for `RESULT.md` instead.

## Completion and handoff

- [ ] Acceptance checks have evidence, and related tests and documentation are
  updated within scope.
- [ ] Unrelated changes, secrets, and environment files are excluded.
- [ ] The handoff includes the issue link, changes, verification, and remaining
  risks or blockers. Publication follows the assignment, not an automatic merge.

Human review and an `AI-Eval:` comment on the PR follow publication; the coder
does not write a human evaluation or grant itself additional permissions.

## Files allowed
- `<relative/path.ext>`

## Ask
<Copy the original Ask verbatim.>
