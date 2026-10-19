# Prompt 04 - full Rust migration

Ray explicitly approved the full rewrite on 2026-10-15, following the earlier
read-only proposal. This is the current prompt. Product trace: FEATURE_SPEC
5.1-5.8, especially 5.4 execution, 5.5 verification and 5.7 the human interface;
sections 3, 7 and 8 still bind the migration.

## Outcome

Replace Roster-owned production JavaScript with native Rust, incrementally and
without changing observable delivery behavior or creating a second runtime
or board. The contracts pack remains a separate GitHub publishing SDK.
Starting the migration is not finishing it.

## Procedure

1. Ray explicitly directs this feature branch to remove the Node implementation,
   not keep it operational alongside Rust. Main and Git history are the legacy
   reference. Do not preserve Node oracle changes, copy a legacy tree, create a
   frozen reference checkout or silently delegate missing behavior to Node.
2. Start with pure leaves, then state and execution boundaries, then transport,
   seats and orchestration, then the shell/CLI cutover. Give every replacement
   an explicit input/output/error contract and existing independent oracle.
3. Port behavioral expectations from the original tests and Git history into
   native Rust unit and integration tests. Preserve persistence/wire bytes,
   malformed-input controls and exact CLI outputs. Do not relax an assertion
   to excuse a mismatch or present removed/unported checks as passing.
4. Preserve persistence versions/ownership, wire formats, unknown usage,
   cancellation/revocation, death-proof locks and platform containment.
5. Add Rust language and Cargo skills/instructions that reach both Copilot
   discovery and Roster's selected task context. No new Rust code comments
   except required license/copyright notices.
6. Use Rust's standard library first. A third-party crate requires a concrete
   capability the standard library cannot safely provide, a reviewed license,
   a locked dependency graph and an explicit manifest change. No framework
   or async runtime is selected by default. Do not add npm runtime dependencies.
7. Run formatting, strict Clippy, native Rust unit and integration tests for
   each port. Retain test failures, unported behavior/coverage and platform gaps.
   Benchmark startup, bridge cost, memory and throughput before claiming gains.

## Initial surfaces

Cargo manifest/lock, Rust source and tests, offline build/test scripts,
scoped instructions and skills, their Roster loading integration, and directly
related documentation. Expand into the corresponding production modules only
when their native replacement is testable. No policy, workflow or vendor edits.

## Release gates

The full rewrite authorization does not approve a live model/hardware campaign,
invent human acceptance, or waive independent review. Cross-platform binary
distribution and release workflow changes remain human-owned. Publication is
only through the reviewed App SDK with complete credentials and eligible review;
HTTP 422 stops publication without a human-credential workaround.
