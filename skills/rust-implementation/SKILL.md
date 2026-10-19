---
name: rust-implementation
description: "Use when implementing, porting, testing or reviewing the native Rust Roster runtime and its behavioral contracts."
---

# Rust implementation

## When to use

Select for Rust implementation, behavior-preserving ports and native review.

## Procedure

1. Read AGENTS.md, prompt 04, TASK.md and the scoped Rust/Cargo instructions.
2. Identify the input/output/error contract in main/Git history and port its
   acceptance cases into native tests. Do not preserve a Node implementation.
3. Implement the smallest correct native behavior with explicit types and
   ownership, bounded input and actionable errors. No unsafe Rust, speculative
   abstractions, silent fallback, or external-input unwrap/expect/panic.
4. Add no code comments except required license/copyright notices. Use clear
   names, tests and separate documentation. Preserve UTF-16, safe-integer,
   path, persistence, cancellation and authority semantics.
5. Run Cargo formatting, strict all-target Clippy and relevant native unit/
   integration tests. Record actual exits, failing controls and platform gaps.
   Consumer JavaScript checks do not substitute for verification of Rust.
6. Keep review independent and assertions strong. No Node fallback, parallel
   runtime or frozen legacy checkout; main already preserves the old source.

## Stop condition

Report incomplete behavior, failing checks and blockers honestly. This skill
grants no tools, scope expansion, policy, workflow, model or publisher authority.
