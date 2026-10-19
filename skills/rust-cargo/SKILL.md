---
name: rust-cargo
description: "Use when building, linting, testing or packaging Rust, changing Cargo manifests, or evaluating a native framework or dependency."
---

# Rust Cargo

## When to use

Select for Cargo builds, Rust dependencies, native frameworks and packaging.

## Procedure

1. Read root AGENTS.md and the scoped Cargo instructions. Use the existing
   library/CLI layout, Rust 2024 and minimum Rust 1.85.
2. Prefer the standard library and no framework. Justify a required crate by
   missing capability, license and transitive/build cost before adding it.
3. Commit the lockfile; keep target output ignored and tools isolated. Do not
   edit global PATH/toolchains, workflow files or vendor sources.
4. Run `cargo fmt --all -- --check`, strict all-target Clippy and locked/offline
   native unit/integration tests. Missing tools fail explicitly. Port checks
   from Git history without retaining a Node implementation or oracle checkout.
5. Measure startup, memory, throughput, clean/incremental builds and artifact
   size separately. Do not conflate language gains with topology changes.
6. Use the native `roster` binary on this branch. Keep the contracts publisher
   independent; deletion does not waive compatibility, review or release gates.

## Stop condition

Do not claim a complete port, platform proof or release from a successful local
build. No packet grants network, live model calls, credentials or publishing.
