---
description: "Use when implementing, porting, testing or reviewing Rust code in the Roster runtime."
applyTo: "rust/**/*.rs"
---

# Rust language

- Follow root AGENTS.md and prompt 04. Preserve behavior, error shapes,
  persistence and authority; port original expectations from main/Git history.
  Do not retain a Node implementation or frozen oracle checkout on this branch.
- New Rust code has no explanatory, inline or documentation comments.
  Retain required license/copyright notices. Use names, types, tests and docs.
- Model actual invariants with small types and explicit ownership. Prefer
  straightforward functions and the standard library over traits, generics,
  frameworks or configuration without a concrete consumer.
- Keep input/output bounded. Check integer ranges, encoding, paths and all
  external input before use. JavaScript UTF-16 and safe-integer semantics
  need explicit parity tests; lossy UTF-8 conversion is not equivalent.
- Return typed, actionable errors. Never use `unwrap`, `expect`, panic,
  unchecked indexing or a success-shaped fallback on fallible external data.
- Unsafe Rust is forbidden. Do not add FFI, an executor, shared mutable state
  or a dependency just to make a port shorter.
- Keep cancellation revocable, wait for owned cleanup and reject late results.
  Lock age is not death proof. Retain one writer per worktree.
- Optimize measured bottlenecks. Avoid unnecessary allocations, copies and
  process hops without weakening validation, review or provenance.
- Run Cargo format, strict all-target Clippy, native unit and integration tests.
  Report exact platforms, unported checks and gaps; deletion is not parity.
