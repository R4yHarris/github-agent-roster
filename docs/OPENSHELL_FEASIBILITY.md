# NVIDIA OpenShell integration: feasibility and capability evaluation

**Status: APPROVED OFFLINE & SANDBOX CONTRACT, 2026-10-23.** Grounded in
[FEATURE_SPEC](FEATURE_SPEC.md) sections 3, 5.1–5.8, and 7, and tracked under
initiative `#675` (`#676`–`#679`).

## 1. Executive feasibility summary

**Verdict: Highly feasible and architecturally complementary — with a strict
host-supervisor boundary.**

NVIDIA OpenShell (`v0.1.x`, Apache-2.0, Rust-native) solves the exact physical
containment and credential-isolation gap identified in
[COPILOT_WORKER_DESIGN.md](COPILOT_WORKER_DESIGN.md) section 4 while
reinforcing Roster's core principles:

| Dimension | Roster requirement ([FEATURE_SPEC](FEATURE_SPEC.md)) | NVIDIA OpenShell (`v0.1.x`) capability | Integration disposition |
| --- | --- | --- | --- |
| **Filesystem containment** (5.4, 5.8) | One writer per worktree; deny `.git`, `.roster`, `vendor/github-agent-contracts`, `agent-policy.yml`, `.github/workflows`, parent `HOME`, and secrets. | `filesystem_policy` (`include_workdir: false`, `read_only`, `read_write`, max 256 paths) enforced in-kernel via Landlock ABI v3+ with `landlock: { compatibility: hard_requirement }`. | **Supported**: Roster compiles task allow-lists directly into `version: 1` OpenShell YAML policies and refuses `best_effort` Landlock fallback. |
| **Process privilege** (5.1, 5.4) | Never run untrusted agent or test workloads as root or with host user credentials. | `process` section (`run_as_user`, `run_as_group`, UID/GID `1..=4294967294`, root `0` rejected). | **Supported**: Roster enforces explicit non-root numeric UID/GID or `sandbox` guest identity. |
| **Network fence & binary attribution** (3, 5.4, 5.8) | Default-deny egress; internet is opt-in; only approved binaries may reach approved inference endpoints. | Outer network fence blocks all direct workload egress except the private supervisor channel; `network_policies` bind allowed endpoints (`host`, `port`, `protocol`, `access`) to specific `binaries`. | **Supported**: Roster generates zero-egress policies for offline verification and binary-scoped inference policies for external worker attempts. |
| **Zero-knowledge credential routing** (5.1, 5.2, 5.8) | Endpoint keys live in the host Vault; GitHub App PEM never enters a worker; credentials must be revocable on cancellation. | `providers` and `profiles` (`category: inference`) inject an opaque placeholder env var into the sandbox; the trusted supervisor substitutes the real credential only on authorized outbound requests, and `provider detach --wait` revokes access immediately. | **Supported**: Roster maps `.roster/fleet.yml` profiles (`127.0.0.1`/`localhost` mapped to `host.openshell.internal`, LAN DGX Spark, Aperture, or cloud endpoints) into OpenShell inference provider profiles. |
| **Formal policy verification** (5.4, 5.5) | Never widen authority during task execution or policy reload. | `openshell-prover` formally verifies that a candidate policy grants no more access than a boundary policy (flagging new hosts, credentialed reach, write methods, or cloud metadata endpoints). | **Supported**: Roster includes a native deterministic boundary prover (`prove_openshell_policy_boundary`) and supports external `openshell-prover` checks. |
| **Cargo dependency hygiene** (5.4, [prompt 04](../prompts/04-rust-migration.md)) | Rust 2024, MSRV 1.85, locked offline Cargo graph, minimal dependencies. | `openshell-sdk` is consumed from git and pulls `tonic`/`prost`/gRPC transport crates. | **Boundary enforced**: Roster does **not** add `openshell-sdk` to `Cargo.toml`. Roster compiles/validates OpenShell YAML policies, provider profiles, and boundary proofs natively in `rust/openshell.rs` and drives a pinned external `openshell` CLI binary under owned process/cancellation control. |

## 2. Platform and operational constraints

1. **Kernel & runtime prerequisites**:
   - **Linux**: Native support with Docker, Podman, MicroVM, or Kubernetes and Linux kernel Landlock ABI v3+.
   - **macOS (Apple Silicon)**: Supported via OpenShell's Linux VM/container runtime.
   - **Windows**: OpenShell runs inside **WSL 2** (with Docker/Podman or MicroVM inside WSL 2), while Windows Job Objects (`process-wrap` + `win32job`) remain Roster's native Windows process-tree owner.
2. **Telemetry default**:
   - OpenShell gateways enable anonymous operational telemetry by default unless `OPENSHELL_TELEMETRY_ENABLED=false` is set. Roster's generated gateway/launch environment always sets `OPENSHELL_TELEMETRY_ENABLED=false`.
3. **Identity & publication boundary**:
   - OpenShell is an **execution and inference sandbox**, never a GitHub publisher or second Kanban board.
   - The GitHub App (`vendor/github-agent-contracts/scripts/agent-pr.mjs`) remains strictly on the host outside OpenShell sandboxes. No GitHub App private key or installation token is ever registered as an OpenShell provider or mounted into a sandbox.
