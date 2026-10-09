# Testing

Tests use only Node's built-in `node --test` runner (Node 20+), with no API key,
model endpoint, or runtime package.

## Running

```sh
npm test                                   # whole suite, parallel, slowest files first
npm test -- tests/tools.test.mjs           # selected files through the same runner
node --test tests/tools.test.mjs           # one file, plain Node
```

`npm test` runs [scripts/run-tests.mjs](../scripts/run-tests.mjs). It runs each
`tests/*.test.mjs` file in its own `node --test` process, using a pool of
workers:

- **Workers:** all CPU cores but one. Set `ROSTER_TEST_JOBS=<n>` to change this.
- **Order:** Node sorts the files it is given, so the pool decides the order
  itself. New or unmeasured files start first, then measured files slowest
  first, so the longest file never starts last.
- **Timings:** each file's wall time is stored in
  `<git common dir>/roster-test-timings.json`. All worktrees share it, and it
  is never part of a diff. Timings only affect ordering, never pass or fail.
- **Output:** a passing file prints one line. A failing file prints its full
  output, and the summary lists every failing file and test.
  `ROSTER_TEST_VERBOSE=1` prints every file's output.

## Keeping it fast

Before the builtin coder's final full-suite verification, Roster runs
`node --check` on existing JavaScript files in its concrete planned/repair scope
and files written in the current tool session (including wildcard-scope writes).
Syntax errors return the exact failing path without launching the suite.
Passing this preflight is not test evidence: the full suite still runs.

Test-runner infrastructure errors (including a scoped-command timeout) stop
the coder context with the original error. They are not tool denials that a
model can ignore before requesting a more expensive full suite. Ordinary
nonzero test results still enter the bounded repair loop.

## Reporting exceptions in RESULT.md

The coder's RESULT.md never presents a nonzero full-suite exit as an
unqualified `Checks: PASS`. Two recorded exceptions are reported explicitly:

- **Baseline exceptions:** failures classified outside the change may be
  permitted by the existing gate, but RESULT.md names the affected
  files, states the failure count, and labels the line
  `Checks: PASS (with test exceptions)`. The report states that these failures
  do not count as a clean full-suite pass. It does not claim the base commit
  was tested merely because a baseline exception was supplied.
- **Transient failures:** when the full suite exits nonzero but the failing
  tests pass when rerun alone, the run is classified as passing, yet RESULT.md
  preserves the original full-suite exit code and reports it distinctly from
  the passing rerun: `Transient test exception: full suite exited <n>; rerun
  passed for <files>` plus a `Full suite: exited <n> before isolated rerun classification` line in
  the test evidence.

Failed, blocked, and timed-out verdicts take precedence over exception notes.
Missing isolated-rerun evidence is reported as unavailable, not passing.
Normal passing, failing, blocked, timed-out, and docs-only results keep their
existing reporting, and RESULT.md still passes through the standard secret
redaction.

Scoped verification uses existing explicitly planned test shards instead of
expanding that module into every sibling shard. Without a present planned
shard, it retains the whole module's test family. Final full-suite verification
is unchanged; narrowed development checks never replace the delivery gate.

Each test file runs in one process, so the slowest file sets a lower bound on
wall time, however many cores there are. Two budgets keep files small enough to
spread across workers:

| Budget | Where | Default |
|---|---|---|
| Measured per-file time | `npm test` warning | 60 s (`ROSTER_TEST_FILE_BUDGET_MS`) |
| Lines per test file | [test-layout test](../tests/test-layout.test.mjs), fails in CI | 1000 |

When a file exceeds either budget, split it by topic into shards named
`tests/<module>.<topic>.test.mjs`, for example `tests/builtin.models.test.mjs`.
Move shared fixtures into `tests/helpers/<module>.mjs`. The dotted name is part
of the contract:

- builtin `run_test` treats every shard as covering `src/**/<module>.mjs`, and
  the coder may update any of them, as described in [tools](TOOLS.md);
- hyphenated names such as `tests/tool-call-continues.test.mjs` stay separate
  test files, not shards of `tool`.

The layout test fails if a shard names a module with no source file.

Avoid wall-clock thresholds that machine load can break. When a test checks a
performance budget, judge the best of several attempts. Make waits generous,
and resolve them as soon as the expected event happens.

## CI

CI runs `node --test tests/*.test.mjs` from a human-owned workflow. It gains
from smaller test files through Node's own file concurrency. Switching CI to
`npm test` would be a human workflow change.
