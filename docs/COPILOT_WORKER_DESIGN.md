# Optional Copilot full-agent worker: design proposal

**Status: DESIGN ONLY, 2026-10-13.** Preparation for #571 under #569; neither
issue is declared complete. Ray delegated the architecture choice on 2026-10-13;
the parent selected preparation of an explicitly selected external
**full-agent worker**, not a Copilot transport substituted into the builtin
single-inference interface. No runtime, authentication, dependency exception,
live inference, publication, or containment acceptance is authorized by this
document. The current author's actual model identity is unverified; this
proposal supplies no measured model or usage attribution.

## 1. Decision and evidence

The reviewed offline packet [R1] reports that the SDK transports messages while
the CLI owns an automatic tool/model loop [R2]. One send can request several
inferences before idle. Observing a response and then aborting does not prove
that a subsequent request has not begun. The synthetic callback fixture proves
mapping and scoped Roster reads, not a production CLI handoff or sandbox.

| Alternative | Decision | Reason |
| --- | --- | --- |
| Keep builtin OpenAI-compatible/local execution only | Supported default | Preserves Node 20 ESM and zero runtime dependencies; no Copilot requirement. |
| CLI or SDK as a builtin single-inference adapter | Reject on current evidence | A CLI agent run is not one Roster model turn; no supported one-call stop/tool-result handoff has been proven. |
| Optional external full-agent worker | Recommend for a separately approved implementation | Honest unit of delegation is a bounded task attempt, with its own provenance, external capability boundary and independent Roster review. |
| SDK-driven full-agent worker | Possible later transport choice, not selected now | Better callback/event surfaces may justify dependencies, but do not remove the CLI loop or physical isolation requirements. |
| In-process experimental FFI or a second autonomous scheduler | Reject | Shared process authority defeats the proposed credential separation; a new scheduler/board is not the product. |

Prefer an externally installed, pinned CLI executable for the first future
worker investigation, not an npm dependency in Roster. This is an operational
dependency recommendation, **not approval to install or execute it**. If the
CLI cannot satisfy the scoped capability/result contract, stop; do not widen
permissions, silently adopt the SDK, or reinterpret an agent run as a model
completion.

The existing [GHCP bridge](GHCP.md) still means sequential builtin seats in one
Roster run; this proposal does not reinterpret Copilot chat/subagent names.
[MULTIAGENT](MULTIAGENT.md) already owns isolated child waves, claims and bounded
attempts. A future worker plugs into that Roster-owned lifecycle as an explicit
execution choice after planning, never a second task queue, persisted
scheduler, chatbot crew, or replacement for git. Any external execution mode
must receive explicit approval before changing today's builtin-only contract.

## 2. Spec trace and non-goals

| Feature | Proposed responsibility |
| --- | --- |
| [5.1](FEATURE_SPEC.md#51-identity-and-authority) | App-only publication outside the worker; fail closed on unknown authoring model. |
| [5.2](FEATURE_SPEC.md#52-roster-and-staffing) | Explicit worker/model selection, declared capacity; no silent fallback. |
| [5.3](FEATURE_SPEC.md#53-work-breakdown) | One immutable Ask/TASK, named outcome, exact file scope and acceptance checks per attempt. |
| [5.4](FEATURE_SPEC.md#54-execution) | Fresh isolated worker filesystem, bounded tools and deadlines, verified artifact import; docs skip suites. |
| [5.5](FEATURE_SPEC.md#55-review-and-gates) | Independent Roster read-only review and trusted checks bound to the exact candidate. |
| [5.6](FEATURE_SPEC.md#56-feedback-and-learning) | Separate full-agent attempt provenance; unknown metrics remain unknown, not human acceptance. |
| [5.7](FEATURE_SPEC.md#57-human-interface) | Reuse the existing status rail and explicit blocked/cancelled states, not a separate UI. |
| [5.8](FEATURE_SPEC.md#58-operate-it) | Revocable grants, wall-clock cap, cleanup evidence, credential and log separation. |

Sections [3](FEATURE_SPEC.md#3-principles) and
[7](FEATURE_SPEC.md#7-what-not-to-build) remain binding: default deny, local
hardware first, one writer per worktree, GitHub Issues/PRs as the board,
human-owned policy/workflows, no fake metrics or autonomous production merge.
This proposal is not section 8 harness acceptance, live onboarding/calibration,
an SDK dependency grant, or a policy/workflow/submodule change.

## 3. Delegation and filesystem boundary

Roster remains the supervisor and owner of the issue claim, planning, context
selection, tests, review, learning record and publication handoff:

1. The host verifies a fresh approved base, acquires the existing exclusive
   task claim, and prepares a feature worktree. It freezes the original Ask,
   TASK/acceptance checks, approved read/write files, model request and budgets.
2. It exports only approved, nonsecret source/context to a **new per-attempt
   worker filesystem**. The worker gets no raw host worktree, `.git` pointer,
   repository-common storage, parent home, sibling tree or publisher mount.
   A normal Git worktree is logical writer separation, not OS isolation:
   its `.git` indirection can expose common repository storage.
3. A distinct approved execution boundary runs the pinned external agent with
   only task-scoped capabilities. No resume/continue session, inherited
   instruction/config/plugin/MCP discovery, fleet fan-out, autopilot or
   default shell/GitHub/browser/network grant is assumed acceptable.
4. The host receives a bounded completion envelope and candidate artifacts.
   It validates paths, types, sizes, deletions, hashes, scope and immutable
   task anchors. Worker narratives and tool logs are untrusted evidence.
5. After worker termination/revocation is verified, the host alone imports
   accepted candidate bytes into its claimed worktree. It rechecks protected
   surfaces and performs trusted verification and independent review.
   A worker cannot edit host TASK, review receipts, policy or publisher inputs.
6. Only a reviewed, unchanged candidate can reach a separately authorized
   App publisher. Failure preserves bounded diagnostics and leaves the issue
   open; it does not generate a new issue or acceptance evaluation.

This initial contract uses strict scope (no worker-initiated expansion).
A future Roster-authorized re-scope requires the existing recorded expansion
and re-estimation process before a new grant, not prompt steering or a
permission dialog inside the worker. Context is bounded and redacted; required
Ask/conduct/checks cannot be silently dropped to fit it.

Prefer brokered scoped Roster callbacks for file/search/edit and explicitly
authorized test requests. The public CLI filters do not establish such a
bridge [R3]. A mock can exercise the proposed contract, but production must
prove the bridge or an equivalent deny-by-default OS-enforced capability set.
Tool names and prompt instructions alone are never enforcement. Exact path
guards must cover absolute/relative escapes, junctions/symlinks/hardlinks,
Windows drive/UNC paths, alternate streams and changes after validation.
Host import must reject special files and unexpected/out-of-scope artifacts.

## 4. Authentication and external capability containment

**Unresolved and live-blocking:** a same-user child with a clean environment
and redirected HOME is not physically isolated. [R1, R4] document token
environment precedence, stored OS keychain credentials and `gh auth token`
fallback. It may still reach the real HOME/USERPROFILE, registry, credential
manager, cached helpers, absolute executables, mounted App keys, inherited
handles, services or sockets. Removing PATH does not prevent absolute-path
execution; changing `COPILOT_HOME` is only configuration/state redirection.

The required boundary is an independently validated dedicated OS principal,
container or VM with explicit filesystem/process/network grants. Choice and
platform support remain unapproved. At minimum, deny:

- Parent HOME/config/credential stores, publishing App PEM and installation
  tokens, human `gh` login, git credential helpers and host repository storage.
- Host IPC/agent sockets, Docker socket, host services/metadata endpoints,
  arbitrary mounted volumes and inherited privileged handles.
- Absolute-path execution of host `gh`, git, shells or credential utilities,
  and indirect access through subprocesses or symlinked/mounted paths.
- Unapproved network destinations, redirects, proxies, DNS-based bypasses,
  telemetry exporters, plugins and MCP servers.

Construct the launch environment from an explicit nonsecret allow-list, not
`process.env` with a blacklist. Offline mocks receive **no auth**. Future live
authentication requires a human-approved, separately scoped inference
credential and revocation/expiry plan, provisioned through a trusted broker
outside model tools. Never inherit the parent login or reuse the publishing
App/token. `useLoggedInUser: false`, if an SDK is later approved, is an
ingredient rather than proof of isolation.

The documented organization server-to-server route [R4] requires its own
organization approval, Copilot Requests permission and installation scope;
it is not permission to expand Roster's publishing App. No route is selected
or tested here. Explicit endpoint/account/model entitlement, repository data
transfer, retention, content capture and telemetry destinations need approval
before any provider request. `COPILOT_OFFLINE=true` is not an air-gap or proof
that BYOK sends no data. Approved inference egress must be distinguished from
general worker internet access.

## 5. Lifecycle, cancellation and budgets

The proposed host-controlled attempt states are prepared, running, stopping,
candidate, reviewing, eligible, or blocked/failed/cancelled. These are local
evidence in existing run records, not a new board. Admission validates finite
wall-clock/output/artifact/tool limits and the approved cost policy before
launch. A pre-cancelled attempt launches nothing. Worker concurrency is one
for the first slice; it does not silently participate in child waves or
best-of-N until a later authorized integration proves their accounting.

Cancellation/deadline first revokes callback/import permissions and prevents
new attempts or publication, then requests agent abort, drains callbacks,
terminates the owned process tree/execution boundary if necessary, and verifies
exit and state cleanup. Late completion/idle/tool events cannot revive success.
Record cleanup errors and retain the claim while a writer may still be alive;
existing positive PID/liveness rules govern crash recovery, not lock age.
Local process exit is not proof of server-side cancellation or zero remaining
charges. Report unresolved remote execution explicitly.

Roster `seat.turn_budget` cannot bound an opaque worker's internal inference
calls. Account separately for full-agent attempts and actually observed tool,
model and cost events. SDK `maxAiCredits` is a post-call soft limit [R5]; one
response can overshoot. No invented CLI `maxTurns: 1`, hard billing ceiling or
universal rate/concurrency limit is allowed. Human spend authorization must
specify units, overshoot tolerance and unknown-usage behavior. If the requested
hard bound cannot be enforced, live admission fails rather than claiming a
wall-clock timeout is a cost ceiling.

## 6. Result, provenance, independent review and publication

The future result envelope must distinguish `external-full-agent` from a
builtin completion. Bind task/run/attempt IDs, base revision, task/scope digest,
approved worker configuration and executable/version digest, candidate file
hashes, observed lifecycle times, exit/cancel/cleanup state and evidence source.
Keep requested model separate from each actually reported served model.
Missing model/usage is unknown; do not fill it from this session, fleet config,
environment, prompt, requested model or a worker's unsupported prose.

An external agent result is **never a completed builtin planner/coder seat**.
Its transcript, reported tool success and idle do not prove tests/review passed.
No legacy builtin last-response metric or eval row may imply otherwise.
Store full-agent totals separately from per-call counts; only sum compatible,
complete, deduplicated reported events. Missing events mean incomplete totals,
not zero. Preserve reported units/schema: a premium multiplier or AI credits
is not USD. Context capacity is declared, not used tokens. Mock evidence is
always synthetic/nonpublishable, including a fully populated model/usage.

Roster performs independent read-only review against the immutable Ask,
acceptance checks, imported bytes/diff and trusted host verification. A worker's
self-review cannot pass this gate. Use existing subject/receipt principles
from [REVIEW](REVIEW.md): bind exact bytes, distinguish observed checks from
model judgments, and fail on missing/stale evidence. A read-only reviewer
cannot write product code, push or publish; trusted verification may execute
approved product checks only within a separately authorized boundary. The
existing Docker verification backend is prior art, not proof of Copilot
credential containment or approval for inference egress.

Publication is an outside-worker host action using the pinned contracts
resolver and `agent-pr.mjs` from the feature worktree root [DEPENDENCY](DEPENDENCY.md).
The host rechecks review, candidate/task digests, branch/base/remote identity,
protected surfaces and actual authoring provenance immediately before handing
off. A later byte/head change invalidates review. Do not borrow the reviewer's
model to publish the worker's edits. Unknown served model blocks publication
under spec 5.1; mixed/unrepresentable authoring models require an explicitly
approved contracts-compatible attribution design, not a convenient substitution.
Human-owned policy/rulesets and exact-head checks remain authoritative.

App credentials exist only in the trusted publisher domain. No direct worker
git commit/push/PR/merge, human credential fallback or review-bypass shortcut is
part of this proposal. Incomplete App credentials stop; HTTP 422 for Checks
permission stops without workaround. Existing GHCP model examples [GHCP](GHCP.md)
are not evidence of this author's or a future worker's actual identity.

## 7. Implementation prerequisites and dependency implications

- The distinct full-agent direction is selected for design. Review its precise
  result/publication attribution contract before implementation; preserve
  builtin defaults and Node 20 ESM.
- Pin/recheck executable/version, documented protocol and applicable license.
  npm CLI requires Node 22+; a native executable still needs platform checks
  (Windows documentation specifies PowerShell 6+) [R6].
- SDK adoption would be a **separate unapproved exception**: the inspected
  snapshot requires Node `^20.19.0 || >=22.12.0` and adds `koffi`,
  `vscode-jsonrpc`, `zod` and optional native runtime packages [R7]. It cannot
  silently narrow Roster's Node >=20 support or zero-runtime-dependency rule.
- Approve/validate the OS boundary, distinct inference auth, account/model/data
  policies, spend/overshoot and retention. No installs, login, discovery,
  entitlement probes or live calls follow from this design.
- Prove production capability control, cancellation and trustworthy results
  before onboarding/calibration; independently review each gate.

## 8. Staged acceptance and bounded next implementation gate

These are gate definitions. Gate B's synthetic development evidence is recorded
below; gates C/D remain unexecuted.

| Gate | Bounded proof and stop condition |
| --- | --- |
| A: docs review (this slice) | Exactly this proposal, spec trace, alternatives and unresolved gates; whitespace/scope check only. No runtime tests or live claims. |
| B: offline mock contract | Separately authorize only a test helper and focused test file, no production import/config/dependency changes. Simulate multi-call agent events, not a fake single inference. |
| B1: authority/files | Explicit snapshot scopes; reject path/special-file escapes, protected surfaces, task mutation, oversize/malformed/duplicate/late results and caller mutation. Assert zero launch/network/auth/publisher capability. |
| B2: lifecycle | Pre-cancel, mid-callback cancel, deadline, missing idle, process/cleanup failure and late events all stay nonpublishable; never release a potentially live writer. |
| B3: provenance | Requested/served mismatch and missing/partial/zero metrics remain distinct; synthetic model evidence cannot become builtin attribution or a publishable AI-Run. |
| B4: review binding | Mock candidate import/review/publication eligibility rejects drift, self-review-only success, stale receipts and wrong task/base; assert no actual publisher invocation. |
| C: containment qualification | After separate approval, test a pinned real executable in the selected OS boundary with synthetic canary credentials and no private task data. Prove denied HOME/keychain/absolute executable/key-mount/IPC/egress paths and process-tree cleanup. Mocks are not this proof. |
| D: authorized live smoke | Only after B/C review and explicit account/data/model/spend approval; one low-risk task, bounded attempt, independent review, no automatic publish. Onboarding/calibration and publication require their own authorization. |

Gate B's test-only follow-up is tracked as #611 after Ray's architecture and
scoped-batch approvals and renewed delivery instruction. Keep gate C's
platform/auth decision separate. Stop on any unsupported contract,
missing proof or unresolved model attribution; do not use the offline packet's
39 passing tests as new worker/isolation acceptance. No second runtime board,
new issues, installs or live data transfer are necessary to decide gate B.

### Gate B development evidence

The test-only [worker helper](../tests/helpers/copilot-worker-offline.mjs) and
[contract tests](../tests/copilot-worker-contract.test.mjs) simulate multiple
inference events per task attempt, rather than treating idle as one inference.
They have no production imports from Roster and do not launch an executable.
They reuse scoped `createTools` reads and protected-path classification.

The synthetic contract freezes caller data, binds Ask/task/base/scope/config
anchors, bounds files/bytes/events/callbacks and JSON structure, and preflights
file/deletion imports. Protected paths, path escapes, ambiguous Windows names,
hardlinks/junctions, malformed/duplicate/oversize envelopes and candidate drift
are refused. Logical-clock deadline, cancellation and revocation deny late
callbacks/import/review; missing idle/exit/cleanup and synthetic cleanup errors
remain blocked. A separate fixture host receipt is required for review
eligibility. Requested/served models, complete/partial/zero/mixed usage units
remain separate, and every mock result is nonpublishable, not a builtin seat
or an AI-Run. The fetch tripwire recorded zero network attempts.

Windows Node `v20.20.2` parent verification ran:

```powershell
& $node20 --test --test-reporter=tap `
  tests\copilot-worker-contract.test.mjs `
  tests\tools.absolute-path.test.mjs tests\tools.test.mjs
```

Result: **121 tests, 120 passed, zero failures, one explicit skip**,
**13087.4816 ms**. The separate child reported 112 passing selected tests on
Node `v20.19.5`; its selector is not equated to this parent selector and its
actual authoring model remains unknown. Parent review added bounded cyclic,
deep, oversized and high-node JSON refusal controls.

These tests prove a proposed fixture protocol only. Empty frozen environment
descriptors are not physical authentication isolation; injected event/exit
flags are not real process or server shutdown. Synchronous fixture import
does not defeat a concurrent filesystem attacker or prove atomic rollback
after I/O failure. A synthetic host receipt is not independent live model
review, and no passing mock authorizes publication. Gates C/D, actual supported
CLI capability wiring, account/data/spend/dependency approval and #570's
publication gate remain separate. No real worker adapter is delivered here.

## Sources and preparation record

No new web research or provider transfer was performed for this proposal.
Public citations below are carried from [R1], checked there on 2026-10-12;
they must be rechecked/pinned before authorized implementation.

- **[R1]** `docs\COPILOT_FEASIBILITY.md`, read-only dependency in
  `D:\oss\github-agent-roster.worktrees\isolated-parallel-sessions-impl`,
  commit `784c04806ca06e75bf0b36af7b56cf388f49d0c4` (#570).
  It is not present in this proposal's base; do not assume it has merged.
  Its parent review records 39/39 offline tests, not live/OS-boundary proof.
- **[R2]** [Official SDK agent-loop guide](https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/docs/features/agent-loop.md).
- **[R3]** [CLI programmatic reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-programmatic-reference)
  and [changing tools](https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/docs/features/changing-tools.md).
- **[R4]** [CLI authentication](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli),
  [SDK authentication](https://docs.github.com/en/copilot/how-tos/copilot-sdk/auth/authenticate),
  [server-to-server tokens](https://docs.github.com/en/copilot/how-tos/copilot-sdk/auth/server-to-server-tokens).
- **[R5]** [Session limits](https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/docs/features/session-limits.md)
  and [usage/billing](https://docs.github.com/en/copilot/how-tos/copilot-sdk/features/usage-and-billing).
- **[R6]** [CLI installation prerequisites](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli).
- **[R7]** [Pinned Node reference](https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/nodejs/README.md)
  and [package metadata](https://github.com/github/copilot-sdk/blob/6be623b01b194a61466385e3f708639ec98ce8ac/nodejs/package.json).

Preparation used fresh fetched `origin/main`
`0652c2c0fb09051544e89b19efe3a7022f93a563` in the exclusive
`agents/copilot-worker-design-20261013` worktree. The base's reviewed contracts
submodule was initialized at `fa48c244bc9b8ead5df927ec705ef41a39c56f68`;
`resolveContractsPath()` selected that local submodule. No publisher was run,
no model identity was substituted, and no issue/project state was changed.
