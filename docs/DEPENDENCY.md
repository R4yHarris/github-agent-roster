# Contracts dependency

[github-agent-contracts](https://github.com/R4yHarris/github-agent-contracts)
provides identity, policy, trailers, and the publishing script. This repository
requires it as the Git submodule at
[`vendor/github-agent-contracts`](../vendor/github-agent-contracts), pinned to
the latest reviewed upstream commit, which refuses missing or unknown model IDs
and validates optional orchestration permissions. Keep the submodule pointer; do
not copy contracts source into this
repository or rewrite files inside the submodule.

## Checkout

Clone with submodules:

```sh
git clone --recurse-submodules https://github.com/R4yHarris/github-agent-roster.git
cd github-agent-roster
```

For an existing checkout or a new worktree, initialize the pinned dependency
from that checkout's root:

```sh
git submodule update --init --recursive
```

This branch builds the native Rust binary with Cargo. The former npm entry
point has been removed; the contracts publishing SDK remains a separate Node
tool. Native local TASK/Ask delivery and App-authenticated read-only issue/status
observations are implemented; GitHub task delivery and publication adapters
remain pending.

Online issue observations use a small owned identity adapter around the SDK's
public `parseOrigin` and `mintInstallationToken` exports, never a copied crypto
or policy implementation. `ROSTER_NODE20` is explicit and version-checked by
the adapter. It receives bounded input without setting the SDK module as the
CLI entry point, and does not inherit human tokens. Rust owns subsequent
fixed-scope HTTPS, validation, cancellation and token revocation.
The SDK's existing contents/PR-write token baseline remains unchanged; only
additional `issues: read` is requested. This does not grant a local policy
capability or change installation permissions. Missing App credentials or
unaccepted permissions fail without a human-authentication fallback.

Native worktree preparation does **not** fetch `.gitmodules` URLs or blindly run
recursive update. Every declared gitlink needs an explicit approved absolute
local source (`ROSTER_LOCAL_SUBMODULES` in [INSTALL](INSTALL.md)); its HEAD must
exactly equal the selected gitlink. Git-only local materialization finishes under
the owned root lease before assignment/planning. Nested paths are explicit and
bounded, source packs and URLs remain unchanged, and missing/stale approvals
stop. A worktree with no declared gitlinks needs no submodule operation.

The installation dependency must contain `scripts/agent-pr.mjs`, be clean at
the installation's reviewed gitlink and retain its physical source identity.
A configured/sibling fallback must meet the same reviewed pin. It does not
replace a worktree-relative declared dependency needed by consumer tests.

If tests are blocked only by missing contracts scripts, the harness prints
`Contracts submodule was not initialized`, not a vendor directory listing.
This is an infrastructure blocker, not a slice test failure: no code repair
budget is consumed, RESULT records `Checks: BLOCKED`, and review/publication
cannot pass until the dependency is initialized. Mixed failures and unrelated
missing modules retain normal failed-test handling.

## Resolution

[`resolve_contracts_path`](../rust/paths.rs) returns the first usable contracts
directory in this order:

1. `vendor/github-agent-contracts` inside the roster checkout.
2. `GITHUB_AGENT_CONTRACTS`, when set. Relative values resolve against the
   caller's current working directory; absolute paths are normalized.
3. `../github-agent-contracts`, the sibling clone beside the roster checkout.

The submodule and sibling locations are relative to the roster installation,
not the caller's current working directory. A candidate is usable only when
`scripts/agent-pr.mjs` is a file. Missing or incomplete candidates are skipped;
if none contains that file, resolution fails with the searched locations and
the submodule initialization command.

`GITHUB_AGENT_CONTRACTS` is optional, but if set it must be a non-empty string.
Blank or non-string values are errors, even when the submodule is initialized.
Unset the variable rather than setting it to an empty value.

## Publish

With `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY_PATH` set, publish from the
current feature worktree's repository root using the initialized submodule:

```powershell
node vendor\github-agent-contracts\scripts\agent-pr.mjs --message "<subject plus Model, Summary, and how-to-test sections>" --model GPT-6.1-Sol --merge-when-green
```

The publisher also requires a feature branch and a human-owned root
`agent-policy.yml` granting the coder publishing capabilities and, for
`--merge-when-green`, the merger capability, matching the reviewed policy on
origin's default branch. Do not create or edit policy as part of dependency
setup.

A human may explicitly initialize a manually created worktree with
`git submodule update --init --recursive` from its root. Native-created local
worktrees use only the approved local-source path described above, not URL
fetch. The native CLI's `--publish`/GitHub issue paths are currently unavailable
before effects; no SDK call, draft PR or model-free publication command is
substituted. Eventual managed publication still requires current passing
private REVIEW authority (or an explicitly allowed skip), reviewed task files,
the complete App credentials and SDK execution from the feature worktree root.
Every Roster publish path passes `--model` explicitly. Completed seats supply
their actual response-backed model and usage. Without a completed seat, GHCP
publication requires `AI_MODEL` or explicit `/publish --model MODEL`, uses
`github-copilot`, and omits used/out; configured served models are not GHCP models.
An absent or invalid ID aborts with `set model` before invoking the SDK.
Completed runs retain the actual coder model even if configuration changes.
Model-free preparation and stub runs report that publication is unavailable
rather than printing a runnable model-free command.

The contracts publisher keeps its core installation token narrow. For approved
read-only project-management or deployment inspection, the caller may request
additional installation permissions for `actions: read`, `deployments: read`,
`environments: read`, `variables: read`, and
`repository_projects: read` or `organization_projects: read`. These are GitHub
App installation permissions, not entries in the local `agent-policy.yml`;
organization project access is installation-wide. Request only the scopes the
operation needs.

Roster-generated messages include `## Model`, `## Summary`, and how to test,
plus a non-closing issue reference when applicable. For direct SDK publication,
include those sections in `--message`; do not send a subject-only PR body.
This GHCP agent sets `AI_MODEL=GPT-6.1-Sol` and `AI_PROVIDER=github-copilot`;
see the complete [GHCP example](GHCP.md).

Do not commit as the signed-in human when App environment variables are set.
Never commit tokens, private keys, or `.env`.

### Recover an interrupted exact-head publication

The reviewed contracts revision `fa48c244bc9b8ead5df927ec705ef41a39c56f68`
adds explicit resume without a second commit, push or PR. After inspecting the
existing App commit and matching open PR, run from its clean feature worktree:

```powershell
node vendor\github-agent-contracts\scripts\agent-pr.mjs --resume "<exact 40-character existing head SHA>" --merge-when-green
```

Metadata-only recovery omits `--merge-when-green`. Resume preserves the
committed model/run and original bot author/committer; do not supply `--model`,
`--message` or `--files`, or substitute current GHCP metadata for a completed
seat. It rechecks local/remote/PR identity, reviewed policy and all required
exact-head checks, including ruleset-only branch protections.
The reviewed revision `0050ada7184d7ef167310ef25b4312a024069505`
also requests repository-scoped **Administration read** for resume only, so
legacy branch-protection requirements can be inspected. Normal publication
and `--merge-when-green` alone retain their existing token scopes. A human
must grant and accept Administration read on the App installation; approving
the code change does not establish that GitHub accepted the permission.
Missing or conflicting metadata authority, dirty state, real head drift,
missing/merged PR or permission failure stops explicitly. HTTP 403/422 while
requesting the resume token stops without a reduced-scope retry; a Checks
permission HTTP 422 is likewise a stop, not permission to use another token
or human fallback.
Merge remains separately authorized and conservative cleanup can report a
confirmed remote merge with incomplete local worktree cleanup.

## Native vault capability dependencies

FEATURE_SPEC 5.8 requires a real encrypted endpoint-key vault; 5.2/5.4 require
selected credentials in native delivery. The standard library provides neither
AES-256-GCM, a fallible OS CSPRNG, guaranteed key erasure nor safe Windows token
and handle-bound ACL operations. Prompt 04 section 6 authorizes the following
specific dependencies, not a framework or a production helper subprocess.

- RustCrypto `aes-gcm =0.10.3`, `aes =0.8.4`, `ghash =0.5.1` and
  `polyval =0.6.2`: MIT OR Apache-2.0, declared minimum Rust 1.56.
  Defaults and the older RNG graph are disabled. Detached in-place AEAD uses
  the existing allocator. Explicit zeroize feature unification is necessary:
  AES-GCM's own feature clears its temporary GHASH key, but alone does not
  clear expanded AES or POLYVAL/GHASH state. Exact published implementations
  were inspected through docs.rs before acquisition. The NCC audit reported
  no significant findings; this is not proof for every CPU. Constant-time
  multiplication remains a hardware requirement.
- `getrandom =0.4.3` (fallible OS fill) and `zeroize =1.9.1` (RAII raw-key
  buffers), both MIT OR Apache-2.0, minimum Rust 1.85; `base64 =0.23.1`,
  MIT OR Apache-2.0, minimum Rust 1.71. These versions were already locked
  transitively and are promoted directly rather than duplicated.
- Windows-only Daniel Dulaney `windows-permissions =0.2.4`, MIT, edition
  2018 with no declared MSRV. Its exact 2021 published source provides safe
  direct `GetSecurityInfo`/`SetSecurityInfo` wrappers over owned file handles
  and SDDL conversion. Its age and limited maintenance are explicit risks.
  The higher-level `WindowsSecure` helper is not used: its group-dependent
  DACL/SACL flags and unknown object type are unsuitable. External null
  DACLs are inspected through SDDL, not the wrapper's panicking DACL getter.
  Token identity comes from the already pinned `winsafe =0.0.24` effective
  thread token with its additional `advapi` feature, never an environment
  username or file owner. No application unsafe/FFI is introduced.
- Already pinned Unix `rustix =1.1.5` gains `fs` for no-follow,
  directory-relative operations and exclusive complete-file rename. This is
  confined to vault consumers; existing state/settings privacy is unchanged.

The crypto graph adds AEAD/cipher/CTR/GHASH/POLYVAL and their small primitive
dependencies; the Windows wrapper adds `winapi 0.3.9`, `bitflags 1` and the
target-specific GNU import archives.
`subtle 2.6.1` is BSD-3-Clause; its copyright, redistribution and disclaimer
conditions were reviewed alongside the MIT/Apache-2.0 primitive licenses.
The first native GNU link exposed a missing SDK `ktmw32` import-library name
when enabling WinSafe's `advapi` feature. The package's tiny GNU-only build
script supplies a linker-script alias to the **real, unchanged**
`libwinapi_ktmw32.a` already shipped by locked `winapi`'s GNU dependency.
It defines no FFI or fake symbols, executes no helper and changes no system
toolchain. The real archive remains required; MSVC uses its normal SDK.
The locked graph/checksums precede package acquisition. Network access is
limited to this explicit registry acquisition; normal checks remain locked
and offline. Compiler/platform validation and the measured incremental graph
are recorded in [RUST](RUST.md) and [TESTING](TESTING.md), not assumed from
crate metadata.

The final lock contains **199 package versions including the application**
(198 registry versions). Resolving the byte-verified prior direct manifest
with retained existing versions gives 183, so this capability adds **16**
registry versions: 11 AEAD/cipher/hash primitive packages, `bitflags 1`,
`windows-permissions`, `winapi`, and its two target-specific GNU import
archives. `getrandom`, `zeroize` and `base64` are direct promotions of already
locked versions, not additional duplicate families. All previous direct
version pins reproduce their original manifest hash when only these additions
and the two justified feature extensions are removed.

Exact checksums, published source URLs, licenses, declared/undeclared MSRVs
and acquisition status are retained in `.vault-evidence/dependency-review.json`.
Both stable 1.99.1 and Rust 1.85.0 GNU pass the frozen 523-case all-target
suite and strict Clippy; optimized native vault and actual CLI/local controls
also pass. This verifies the selected GNU graph, not unexecuted platform or
hardware claims.

## Trailer CI

The [composite action](../.github/actions/check-agent-trailers/action.yml)
passes the PR base and head SHAs to the pinned submodule's
`scripts/check-pr-agent-trailers.mjs`, rather than the roster's local copy.
The human-owned [workflow](../.github/workflows/check-agent-trailers.yml) must
check out full history with `persist-credentials: false` and
`submodules: recursive`, then use Node 20 before running that action. Do not
check out the base revision before the local action: that replaces the PR
checkout and can leave its checker without required files. The workflow also
needs a separate `tests` job running `node --test tests/*.test.mjs` on Node 20.

Run the Node 20+ ESM tests with `npm test`. No runtime packages are required.
