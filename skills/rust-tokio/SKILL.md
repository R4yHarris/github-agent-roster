---
name: rust-tokio
description: "Use when implementing Tokio futures, cancellation, deadlines, owned process I/O or async transport in the Rust runtime."
---

# Rust Tokio

## When to use

Select for asynchronous process ownership, transport, cancellation and timers.

## Procedure

1. Read root AGENTS.md, prompt 04 and the scoped Tokio instructions.
2. Use the existing current-thread runtime; never create an executor per
   request, tool call or seat. Keep synchronous leaves executor-independent.
3. Register notifications before checking state. Recheck cancellation before
   side effects and terminal success; first cancellation reason wins.
4. Own every child/task through cleanup. Close stdin, bound both output
   streams, kill and await owned children on interruption, and report unknown
   cleanup without claiming release. Child exit is not descendant containment.
5. Do not detach writes, model calls, process drains or cleanup. Join bounded
   work; propagate failures and preserve observed failure evidence.
6. Keep locks out of await points. No unsafe Rust, explanatory comments,
   speculative concurrency, background swarms or extra task boards.
7. Run real cancellation/overflow/late-result controls through native Cargo
   tests. Consumer Node checks cannot substitute for Rust verification.

## Stop condition

Stop on failed ownership, unknown cleanup, policy denial or missing checks.
This packet grants no model endpoint, network, credential or publishing access.
