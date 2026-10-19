---
description: "Use when changing Cargo manifests, Rust dependencies, native builds or Rust packaging."
applyTo: "Cargo.toml,Cargo.lock,rust/**/Cargo.toml,**/.cargo/config.toml"
---

# Cargo and native framework

- The framework is one library and one native CLI. Tokio's current-thread
  executor is selected for cancellable owned process I/O and async transport.
  Read the Tokio instructions; do not add another executor, web/CLI framework
  or FFI layer without an actual migrated behavior that requires it.
- Use Rust 2024, minimum Rust 1.85, and the committed Cargo.lock. Keep generated
  binaries and target output ignored. Do not change the global toolchain/PATH.
- Use `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked --offline -- -D warnings`,
  and `cargo test --all-targets --locked --offline`. Port acceptance cases from
  main/Git history into native tests without weakening expectations.
- Prefer no external crates. Before a necessary manifest change, document the
  missing standard-library capability, license, maintained upstream and
  transitive cost. Never fetch dependencies during an ordinary offline test.
- Dependency/runtime selection does not grant model calls, credentials, network
  tools, workflow edits or App publishing permissions.
- This branch removes the Node application and oracle-preservation scaffolding.
  Use the native `roster` binary; do not add a Node fallback or legacy checkout.
  Keep the independent contracts publisher separate.
- Do not assert Windows ACL or macOS/Linux behavior from a host-injected path
  test. Native platform tests and human-owned binary release gates are distinct.
