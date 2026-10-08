# Repository-local lifecycle hooks

Lifecycle hooks are human-reviewed, read-only gates (FEATURE_SPEC sections
5.4 and 5.5). They add repository-specific checks without exposing a shell tool
to the model, changing agent policy, or adding runtime dependencies.

Commit a manifest at `.roster/hooks.yml` and its Node scripts under
`.roster/hooks/` before starting an issue run:

```yaml
hooks:
  - event: pre-plan
    script: .roster/hooks/prerequisites.mjs
    timeout_ms: 10000
  - event: post-coder
    script: .roster/hooks/product-check.mjs
    timeout_ms: 10000
  - event: pre-publish
    script: .roster/hooks/release-check.mjs
    timeout_ms: 10000
```

The manifest uses the same minimal YAML list parser as other repository
catalogs. Only `event`, `script`, and optional `timeout_ms` are accepted.
`hooks: []` disables hooks. Timeout defaults to 10 seconds, with a maximum of
60 seconds and at most 16 entries. Scripts must be regular, single-link Node
`.js`, `.cjs`, or `.mjs` files directly under `.roster/hooks/`; manifest and
scripts are limited to 64 KiB each. No shell strings, arbitrary executables,
arguments, YAML anchors, or external script paths are supported. Node executes
the scripts explicitly, so executable bits and platform shell syntax are not
required.

The manifest and all declared scripts must match their committed `HEAD`
versions (normalizing Git's LF/CRLF checkout conversion). Uncommitted scripts
are refused. Both the manifest and script directory are protected from planner
and coder tools, including scope expansion and delete. Humans own changes to
these files. Hooks may import application code to check it, but must not write
files, change Git state, spawn background workers, or alter policy. Do not
enable hooks from an untrusted repository.

## Gate behavior

- **pre-plan:** runs before a new planner turn, including planning-only/local
  planning. A failure stops planning before any model request. A reused
  validated handoff does not run a new planner and therefore skips this event.
- **post-coder:** runs after an otherwise passing configured coder candidate,
  before independent review. A nonzero exit or timeout returns a finding
  through the ordinary gate-repair path. One correction per coder context is
  allowed, within the same scope; the hook runs again and must pass. Existing
  bounded perspective/review repairs remain unchanged. Hooks are not run for
  deterministic stub results, which are not verified implementations.
- **pre-publish:** runs in the reviewed feature worktree before staging or
  invoking the App SDK. It also guards manual `/publish` without a Roster run.
  Failure stops publication even with `--skip-review`. The existing final
  excellence/snapshot and review checks still apply; a hook cannot approve a
  failed coder or a losing best-of-N candidate.

Hooks execute sequentially in manifest order; the first failure stops that
event. Successful stdout/stderr is not printed as a finding. On failure the
bounded output becomes a `Lifecycle hook:` finding, with secret redaction.
Output is untrusted evidence, never an instruction to expand scope or bypass
a gate. The coder's final hook statuses appear in `RESULT.md` and the existing
run event stream with measured hook duration. Failure reasons enter normal
local learning records.

Combined stdout/stderr is limited to 4 KiB. Exceeding that limit terminates the
hook and reports an output-limit finding rather than a truncated secret.
Timeout and cancellation terminate the owned process tree (Windows PID-tree
termination; Unix process group). Cancellation stays cancellation, not a
successful or repairable result.

The child environment is an **allow-list**: PATH/PATHEXT, SystemRoot/WINDIR/
COMSPEC, TEMP/TMP, LANG/LC_ALL, and TZ only. App credentials, model keys, Git
credentials/configuration, home-directory variables, and Node preload flags
are not inherited. No secret-bearing context or Ask is passed as an argument.

Worktree snapshots and HEAD/staged-index checks around each hook detect
mutations and fail explicitly;
changes remain visible for inspection, never silently rolled back. This is
**not an OS sandbox**: a trusted Node script can still access the filesystem,
network, and OS under the running user's permissions. Snapshot checks cannot
prevent access to external files or catch every transient side effect. Use
only reviewed, read-only scripts and external OS isolation when required.

With no manifest (or an empty hook list), no hook process or Git trust check
runs and the previous seat/gate behavior is unchanged. No live fleet endpoint
is required to test hooks; use temporary Git repositories and local scripts.
