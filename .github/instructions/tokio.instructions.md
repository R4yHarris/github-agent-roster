---
description: "Use when implementing or reviewing Tokio cancellation, timers, process ownership and async transport."
applyTo: "rust/process.rs,rust/cancel.rs,rust/identity.rs,rust/llm/**/*.rs,rust/execution/**/*.rs,rust/seats/**/*.rs"
---

# Tokio runtime

- Use one current-thread runtime per asynchronous CLI/session invocation.
  Do not instantiate an executor per model request or seat.
- Never hold a synchronous lock across await. Register Notify waiters before
  checking terminal state to avoid lost wakeups.
- Cancellation revokes late success; propagate parent cancellation through
  descendant lifetimes and preserve the first reason.
- Own every child process through kill and wait. Close stdin, bound stdout
  and stderr by bytes and drop drains on interruption. Report cleanup failures
  explicitly; unknown cleanup cannot become a released writer or passing run.
- Killing one child is not proof that descendants or external services stopped.
  Never claim physical containment, hard-spend enforcement or worker recovery
  from a unit test of an owned direct child.
- Do not detach tasks, writes or cleanup, block the event loop on long work,
  or introduce an additional orchestration/runtime layer.
- Use the minimal selected Tokio features and existing native test runner.
  No comments except required notices, unsafe Rust or silent fallback.

## Learnings

Wrong: cleaning cached usage or rolling back response evidence only after
`.await` misses a pending future's `Drop`, because that continuation never executes.
Right: mark aggregate usage unknown before awaiting another request attempt
and defer response publication until observation settles, or use a Drop-safe
ownership guard, while retaining actual parsed evidence when an observer
returns a non-cancellation error.
Never hold an interior-mutability borrow or synchronous lock across `.await`.
Test actual transport/observer readiness barriers, then cancel and immediately
drop the pending future without polling cancellation again; verify prior
evidence, unknown totals and owned cleanup.

Intent is not native submission: mark a private witness only at the actual
spawn attempt, after cancellation, deadline and other preflight gates.
Only that unmarked in-process witness proves no attempt; missing PIDs/CIDs,
absent objects, logs and serialized receipts do not.
Keep ambiguous attempts fail-closed, even when no PID or CID was returned.
Test expiry/cancellation before admission and actual attempts before PID/CID
publication with native readiness barriers.

Keep the original HEAD, branch and actual worktree index sealed through
completion and context handoff; a snapshot taken after drift cannot authorize
pending work.
Revalidate after awaited observers and before each effect or passing result,
not just once before a batch.
Cover linked and split/shared indexes without letting read-only Git refresh
the index.
Test same-commit branch and index-only changes at response, tool and
finalization callbacks, requiring refusal before effects and no fresh-context
or passing receipt.
