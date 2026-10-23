# OpenShell orchestration and library architecture (`rust/openshell.rs`)

**Product trace:** [FEATURE_SPEC](FEATURE_SPEC.md) sections 5.1, 5.2, 5.3, 5.4,
5.5, 5.6, 5.7, and 5.8; honors sections 3 and 7. Tracked under initiative `#675`
(`#676`, `#677`, `#678`, `#679`).

## 1. How Roster supports and empowers OpenShell

Roster empowers OpenShell across four native Rust layers in [`rust/openshell.rs`](../rust/openshell.rs):

### Layer 1: Task-scoped OpenShell v1 policy compiler (`#676`)

`compile_task_policy` deterministically translates a Roster task scope (`read_only`
and `read_write` workspace-relative paths, non-root `run_as_user`/`run_as_group`,
and optional binary-bound inference endpoints) into a validated OpenShell
`version: 1` policy (`OpenShellPolicy`):

- **Hardened defaults**:
  - `version: 1`
  - `filesystem_policy.include_workdir: false` (explicit path grants only; never implicit whole-workdir write access)
  - `landlock.compatibility: "hard_requirement"` (refuses to start if Landlock ABI v3+ cannot enforce the ruleset)
  - `process.run_as_user` / `process.run_as_group`: non-root UID/GID (`1..=4294967294`) or `"sandbox"`
- **Strict path & surface validation**:
  - Every sandbox path must be absolute under `/sandbox/workspace/...` (or `/usr`, `/lib`, `/bin`, `/etc`, `/tmp`), without `..`, null/control bytes, or `/` in `read_write`, and capped at 256 total paths.
  - Protected Roster surfaces (`agent-policy.yml`, `.github/workflows/*`, `vendor/github-agent-contracts/*`, `.git/*`, `.roster/*`, `.env*`, `*.pem`) are unconditionally denied in `read_write` and secret surfaces are denied in `read_only`.
  - Network rule keys starting with `_provider_` are rejected so task policies cannot spoof or override OpenShell provider-contributed rules.

### Layer 2: Fleet & inference provider profile bridge (`#676`)

`compile_fleet_provider_profile` bridges any validated `.roster/fleet.yml`
profile (`Profile`) into an OpenShell `category: inference` provider profile
(`OpenShellProviderProfile`):

- **Host-local endpoint translation**:
  - Local fleet endpoints (`127.0.0.1`, `localhost`, `[::1]`) are automatically mapped to `host.openshell.internal` on their configured port so sandboxed workloads can reach local vLLM/Ollama servers (such as `rtx-3090-qwen` on port `11435`) through the OpenShell supervisor without exposing host loopback sockets.
  - LAN and remote endpoints (such as `aperture.xen.blackmesaresearch.ai:443` or `192.168.1.48:8000`) preserve their exact host, port, and TLS/REST enforcement.
- **Zero-knowledge credential substitution**:
  - If the fleet profile declares `api_key_env`, the OpenShell provider profile declares only the credential key name (`env_var`) and opaque placeholder binding — never the secret value from the Roster Vault or environment.
  - Allowed client binaries (`binaries`) are explicitly enumerated (e.g. `/usr/local/bin/roster-worker`, `/usr/bin/curl`, `/usr/bin/python3`).

### Layer 3: Per-attempt sandbox lifecycle, revocation, and artifact import (`#677`)

`OpenShellSandboxAttempt` manages the host-supervised lifecycle for running
either an external full-agent worker or an isolated verification command inside
OpenShell:

1. **State machine**:
   `prepared -> running -> stopping -> candidate -> reviewing -> eligible | blocked | failed | cancelled`.
2. **Single-link staging (`stage_workspace_files`)**:
   Exports only task-approved, non-symlink, single-hardlink regular files into a
   fresh per-attempt staging directory. No `.git` indirection, `.roster` state,
   Vault files, or App keys are staged.
3. **Revocation-first cancellation (`cancel_and_detach`)**:
   On cancellation or wall-clock deadline, Roster immediately revokes artifact
   import permission, records provider detachment (`openshell sandbox provider detach <sandbox> <provider> --wait`),
   and transitions to `cancelled` (or `failed` if cleanup/detachment is unverified).
   Late completions after cancellation are rejected.
4. **Candidate inspection, host review & artifact emission (`inspect_candidate`, `verify_host_review`, `emit_managed_artifacts`)**:
   Validates candidate file hashes, scope, and immutable task digests, requires an
   independent host review (`verdict: "pass"`) bound to the exact `taskDigest` and
   `candidateDigest`, and emits `RESULT.md` and `REVIEW.md` with `external-openshell-sandbox`
   provenance while preserving `"unknown"` for any unreported token usage.

### Layer 4: Offline readiness diagnostics & formal boundary prover (`#678`, `#679`)

- **CLI & interactive shell diagnostics**:
  - `roster doctor --openshell [--opt-in] [--runtime PATH] [--gateway URL] [--policy PATH] [--provider ID]`
  - `/doctor openshell [--opt-in] [--runtime PATH] [--gateway URL] [--policy PATH] [--provider ID]`
  - Evaluates declared readiness offline without probing networks, starting containers, or printing secret values, reporting deterministic blocker codes (`OPENSHELL_NOT_OPTED_IN`, `OPENSHELL_RUNTIME_UNPINNED`, `OPENSHELL_GATEWAY_UNCONFIGURED`, `OPENSHELL_POLICY_MISSING`, `OPENSHELL_PROVIDER_UNBOUND`).
- **Formal policy boundary prover (`prove_openshell_policy_boundary`)**:
  - Verifies that a candidate `OpenShellPolicy` is a strict subset of a boundary `OpenShellPolicy`:
    - Refuses any Landlock downgrade from `hard_requirement` to `best_effort`.
    - Refuses root process identities or UID/GID drift.
    - Refuses any `read_write` or `read_only` path outside the boundary's allowed prefixes.
    - Refuses any network endpoint host/port/method expansion or cloud metadata IP (`169.254.169.254`).
- **Multi-model sandbox calibration scorecard (`evaluate_openshell_calibration_scorecard`)**:
  - Evaluates M1–M3 calibration runs across fleet profiles under OpenShell containment, requiring passing boundary proofs, verified provider detachment/cleanup, and independent host review before marking any gate passed.
